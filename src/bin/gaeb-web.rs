use std::{
    collections::HashMap,
    env,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::{Context, Result};
use axum::{
    body::{Body, Bytes},
    extract::{DefaultBodyLimit, Multipart, Path as AxumPath, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{Duration as ChronoDuration, Utc};
use gaeb_toolkit::{
    apply_provisional_flags, inject_pdf_pngs, parse_pdf, read_gaeb_xml, write_gaeb_pdf, write_x83,
};
use hmac::{Hmac, Mac};
use lettre::{
    message::{header::ContentType, Attachment, MultiPart, SinglePart},
    transport::smtp::authentication::Credentials,
    Message, SmtpTransport, Transport,
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use scraper::{Html, Selector};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use tokio::{fs, net::TcpListener, sync::RwLock, task};
use tower_http::set_header::SetResponseHeaderLayer;
use tower_http::{services::ServeDir, trace::TraceLayer};
use tracing::{error, info};
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

const DEFAULT_MAX_UPLOAD_BYTES: usize = 2 * 1024 * 1024;
const DEFAULT_PAID_MAX_UPLOAD_BYTES: usize = 25 * 1024 * 1024;
const IMPRINT_URL: &str = "https://www.hawkvision.de/impressum/";
const IMPRINT_CACHE_TTL: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Clone)]
struct AppState {
    data_dir: PathBuf,
    db_path: PathBuf,
    max_upload_bytes: usize,
    paid_max_upload_bytes: usize,
    retention_hours: i64,
    diagnostic_retention_days: i64,
    smtp: Option<SmtpConfig>,
    http_client: reqwest::Client,
    imprint_cache: Arc<RwLock<Option<CachedImprint>>>,
    tracking: PublicTrackingConfig,
    stripe: Option<StripeConfig>,
}

#[derive(Clone)]
struct StripeConfig {
    secret_key: String,
    webhook_secret: String,
    single_price_id: String,
    pro_price_id: String,
    public_base_url: String,
}

#[derive(Clone)]
struct CachedImprint {
    sections: Vec<ImprintSection>,
    cached_at: Instant,
}

#[derive(Clone)]
struct SmtpConfig {
    host: String,
    port: u16,
    username: String,
    password: String,
    from: String,
}

#[derive(Default)]
struct UploadForm {
    email: String,
    contact_name: String,
    company: String,
    phone: String,
    consent: bool,
    email_fallback_consent: bool,
    feature_updates_consent: bool,
    filename: String,
    pdf: Vec<u8>,
}

#[derive(Serialize)]
struct CreateJobResponse {
    id: String,
    token: String,
    status: &'static str,
}

#[derive(Serialize)]
struct JobStatusResponse {
    id: String,
    status: String,
    filename: String,
    error: Option<String>,
    download_url: Option<String>,
    expires_at: String,
}

#[derive(Serialize)]
struct ErrorResponse {
    error: String,
}

#[derive(Serialize)]
struct BillingConfigResponse {
    enabled: bool,
    single_net_cents: u32,
    pro_net_cents: u32,
}

#[derive(Serialize)]
struct BillingStatusResponse {
    signed_in: bool,
    plan: &'static str,
    single_credits: i64,
    max_upload_bytes: usize,
}

#[derive(Deserialize)]
struct CheckoutRequest {
    offer: String,
    email: String,
}

#[derive(Serialize)]
struct CheckoutResponse {
    url: String,
}

#[derive(Clone, Serialize)]
struct ImprintSection {
    heading: String,
    lines: Vec<String>,
}

#[derive(Serialize)]
struct ImprintResponse {
    sections: Vec<ImprintSection>,
    source_url: &'static str,
}

#[derive(Clone, Default, Serialize)]
struct PublicTrackingConfig {
    google_tag_manager_id: Option<String>,
    google_analytics_id: Option<String>,
    meta_pixel_id: Option<String>,
    klicktipp_pixel_url: Option<String>,
    consent_version: String,
}

struct JobRecord {
    id: String,
    token: String,
    filename: String,
    status: String,
    error: Option<String>,
    expires_at: String,
    email: String,
    contact_name: String,
    email_fallback_consent: bool,
    billing_tier: String,
}

struct BillingAccess {
    email: String,
}

struct PurchaseNotice {
    email: String,
    offer: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let data_dir = PathBuf::from(env::var("DATA_DIR").unwrap_or_else(|_| "data".to_owned()));
    fs::create_dir_all(data_dir.join("jobs")).await?;
    let state = Arc::new(AppState {
        db_path: data_dir.join("gaeb-web.sqlite3"),
        data_dir,
        max_upload_bytes: env_usize("MAX_UPLOAD_BYTES", DEFAULT_MAX_UPLOAD_BYTES),
        paid_max_upload_bytes: env_usize("PAID_MAX_UPLOAD_BYTES", DEFAULT_PAID_MAX_UPLOAD_BYTES),
        retention_hours: env_i64("RETENTION_HOURS", 24),
        diagnostic_retention_days: env_i64("DIAGNOSTIC_RETENTION_DAYS", 30),
        smtp: smtp_config(),
        http_client: reqwest::Client::builder()
            .user_agent("GAEB-Konverter/0.1 (+https://gaeb.hawk-vision.de)")
            .timeout(Duration::from_secs(12))
            .build()?,
        imprint_cache: Arc::new(RwLock::new(None)),
        tracking: tracking_config(),
        stripe: stripe_config(),
    });
    init_database(&state)?;
    for notice in backfill_stripe_events(&state)? {
        if let Err(error) = issue_billing_access_email(&state, &notice) {
            error!(%error, email = %notice.email, "billing access email could not be sent");
        }
    }

    let cleanup_state = state.clone();
    tokio::spawn(async move {
        loop {
            if let Err(err) = cleanup_expired_jobs(&cleanup_state).await {
                error!(error = %err, "cleanup failed");
            }
            tokio::time::sleep(Duration::from_secs(3600)).await;
        }
    });

    let app = Router::new()
        .route("/health", get(health))
        .route("/api/convert", post(create_job))
        .route("/api/gaeb-to-pdf", post(gaeb_to_pdf))
        .route("/api/legal/imprint", get(imprint))
        .route("/api/public-config", get(public_config))
        .route("/api/billing/config", get(billing_config))
        .route("/api/billing/status", get(billing_status))
        .route("/api/billing/checkout", post(create_checkout))
        .route("/api/stripe/webhook", post(stripe_webhook))
        .route("/billing/access/{token}", get(activate_billing_access))
        .route("/api/jobs/{id}", get(job_status))
        .route("/download/{id}/{token}", get(download))
        .fallback_service(ServeDir::new("web").append_index_html_on_directories(true))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::HeaderName::from_static("permissions-policy"),
            HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::REFERRER_POLICY,
            HeaderValue::from_static("strict-origin-when-cross-origin"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_FRAME_OPTIONS,
            HeaderValue::from_static("DENY"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(DefaultBodyLimit::max(
            state.paid_max_upload_bytes + 256 * 1024,
        ))
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let bind = env::var("BIND").unwrap_or_else(|_| "0.0.0.0:8080".to_owned());
    let address: SocketAddr = bind.parse().context("BIND ist keine gültige Adresse")?;
    let listener = TcpListener::bind(address).await?;
    info!(%address, "GAEB web service started");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health() -> &'static str {
    "ok"
}

async fn public_config(State(state): State<Arc<AppState>>) -> Json<PublicTrackingConfig> {
    Json(state.tracking.clone())
}

async fn billing_config(State(state): State<Arc<AppState>>) -> Json<BillingConfigResponse> {
    Json(BillingConfigResponse {
        enabled: state.stripe.is_some(),
        single_net_cents: 990,
        pro_net_cents: 1900,
    })
}

async fn billing_status(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
) -> Result<Json<BillingStatusResponse>, ApiError> {
    let Some(access) =
        billing_access_from_headers(&state, &headers).map_err(|_| ApiError::internal())?
    else {
        return Ok(Json(BillingStatusResponse {
            signed_in: false,
            plan: "free",
            single_credits: 0,
            max_upload_bytes: state.max_upload_bytes,
        }));
    };
    let connection = Connection::open(&state.db_path).map_err(|_| ApiError::internal())?;
    let (pro_active, single_credits): (bool, i64) = connection
        .query_row(
            "SELECT pro_active, single_credits FROM billing_accounts WHERE email = ?1",
            [&access.email],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
        .map_err(|_| ApiError::internal())?
        .unwrap_or((false, 0));
    Ok(Json(BillingStatusResponse {
        signed_in: true,
        plan: if pro_active { "pro" } else { "single" },
        single_credits,
        max_upload_bytes: state.paid_max_upload_bytes,
    }))
}

async fn create_checkout(
    State(state): State<Arc<AppState>>,
    Json(request): Json<CheckoutRequest>,
) -> Result<Json<CheckoutResponse>, ApiError> {
    let stripe = state
        .stripe
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("Der Bezahlbereich ist noch nicht freigeschaltet."))?;
    let email = request.email.trim().to_lowercase();
    if !valid_email(&email) {
        return Err(ApiError::bad_request(
            "Bitte eine gültige E-Mail-Adresse angeben.",
        ));
    }
    let (price_id, mode) = match request.offer.as_str() {
        "single" => (&stripe.single_price_id, "payment"),
        "pro" => (&stripe.pro_price_id, "subscription"),
        _ => return Err(ApiError::bad_request("Unbekanntes Angebot.")),
    };
    let success_url = format!(
        "{}/?checkout=success&session_id={{CHECKOUT_SESSION_ID}}",
        stripe.public_base_url
    );
    let cancel_url = format!("{}/?checkout=cancelled#preise", stripe.public_base_url);
    let mut checkout_fields = vec![
        ("mode", mode),
        ("line_items[0][price]", price_id.as_str()),
        ("line_items[0][quantity]", "1"),
        ("customer_email", email.as_str()),
        ("success_url", success_url.as_str()),
        ("cancel_url", cancel_url.as_str()),
        ("billing_address_collection", "required"),
        ("tax_id_collection[enabled]", "true"),
        ("automatic_tax[enabled]", "true"),
        ("metadata[offer]", request.offer.as_str()),
    ];
    if mode == "payment" {
        checkout_fields.push(("customer_creation", "always"));
    }
    let response = state
        .http_client
        .post("https://api.stripe.com/v1/checkout/sessions")
        .bearer_auth(&stripe.secret_key)
        .form(&checkout_fields)
        .send()
        .await
        .map_err(|error| {
            error!(%error, "stripe checkout request failed");
            ApiError::upstream("Stripe ist vorübergehend nicht erreichbar.")
        })?;
    let status = response.status();
    let response_body = response.text().await.map_err(|error| {
        error!(%error, "stripe checkout response could not be read");
        ApiError::upstream("Stripe hat eine ungültige Antwort geliefert.")
    })?;
    let value: serde_json::Value = serde_json::from_str(&response_body).map_err(|error| {
        error!(%error, "stripe checkout response was not JSON");
        ApiError::upstream("Stripe hat eine ungültige Antwort geliefert.")
    })?;
    if !status.is_success() {
        error!(%status, response = %value, "stripe checkout rejected");
        return Err(ApiError::upstream(
            "Der Stripe-Checkout konnte nicht geöffnet werden.",
        ));
    }
    let url = value
        .get("url")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| ApiError::upstream("Stripe hat keine Checkout-Adresse geliefert."))?;
    Ok(Json(CheckoutResponse {
        url: url.to_owned(),
    }))
}

async fn stripe_webhook(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<StatusCode, ApiError> {
    let stripe = state
        .stripe
        .as_ref()
        .ok_or_else(|| ApiError::unavailable("Stripe ist nicht konfiguriert."))?;
    let signature = headers
        .get("stripe-signature")
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| ApiError::bad_request("Stripe-Signatur fehlt."))?;
    verify_stripe_signature(signature, &body, &stripe.webhook_secret)?;
    let event: serde_json::Value = serde_json::from_slice(&body)
        .map_err(|_| ApiError::bad_request("Ungültiges Stripe-Ereignis."))?;
    let event_id = event
        .get("id")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| ApiError::bad_request("Stripe-Ereignis ohne ID."))?;
    let event_type = event
        .get("type")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("unknown");
    let payload = String::from_utf8_lossy(&body).into_owned();
    let db_path = state.db_path.clone();
    let event_id = event_id.to_owned();
    let event_type = event_type.to_owned();
    let stored_event_id = event_id.clone();
    let stored_event_type = event_type.clone();
    let notice = task::spawn_blocking(move || {
        record_and_apply_stripe_event(&db_path, &stored_event_id, &stored_event_type, &payload)
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(|error| {
        error!(%error, "stripe event could not be recorded");
        ApiError::internal()
    })?;
    if let Some(notice) = notice {
        let mail_state = state.clone();
        task::spawn_blocking(move || {
            if let Err(error) = issue_billing_access_email(&mail_state, &notice) {
                error!(%error, email = %notice.email, "billing access email could not be sent");
            }
        });
    }
    info!(%event_id, %event_type, "stripe event recorded and applied");
    Ok(StatusCode::OK)
}

async fn activate_billing_access(
    State(state): State<Arc<AppState>>,
    AxumPath(token): AxumPath<String>,
) -> Result<Response, ApiError> {
    let raw_session = Uuid::new_v4().simple().to_string();
    let token_hash = hash_token(&token);
    let session_hash = hash_token(&raw_session);
    let db_path = state.db_path.clone();
    task::spawn_blocking(move || activate_access_token(&db_path, &token_hash, &session_hash))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|error| {
            error!(%error, "billing access token could not be activated");
            ApiError::bad_request("Der Zugangslink ist ungültig oder abgelaufen.")
        })?;
    let cookie = format!(
        "gaeb_session={raw_session}; Path=/; Max-Age=2592000; HttpOnly; Secure; SameSite=Lax"
    );
    let mut response = Redirect::to("/?billing=ready#preise").into_response();
    response.headers_mut().insert(
        header::SET_COOKIE,
        HeaderValue::from_str(&cookie).map_err(|_| ApiError::internal())?,
    );
    Ok(response)
}

fn verify_stripe_signature(signature: &str, body: &[u8], secret: &str) -> Result<(), ApiError> {
    let mut timestamp = None;
    let mut signatures = Vec::new();
    for part in signature.split(',') {
        if let Some(value) = part.strip_prefix("t=") {
            timestamp = value.parse::<i64>().ok();
        } else if let Some(value) = part.strip_prefix("v1=") {
            signatures.push(value);
        }
    }
    let timestamp = timestamp.ok_or_else(|| ApiError::bad_request("Ungültige Stripe-Signatur."))?;
    if (Utc::now().timestamp() - timestamp).abs() > 300 {
        return Err(ApiError::bad_request("Abgelaufene Stripe-Signatur."));
    }
    let signed = format!("{timestamp}.{}", String::from_utf8_lossy(body));
    let valid = signatures.iter().any(|signature| {
        let Ok(expected) = decode_hex(signature) else {
            return false;
        };
        let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else {
            return false;
        };
        mac.update(signed.as_bytes());
        mac.verify_slice(&expected).is_ok()
    });
    if !valid {
        return Err(ApiError::bad_request("Stripe-Signatur nicht gültig."));
    }
    Ok(())
}

fn decode_hex(value: &str) -> Result<Vec<u8>, ()> {
    if value.len() % 2 != 0 {
        return Err(());
    }
    (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16).map_err(|_| ()))
        .collect()
}

fn record_and_apply_stripe_event(
    db_path: &Path,
    id: &str,
    event_type: &str,
    payload: &str,
) -> Result<Option<PurchaseNotice>> {
    let mut connection = Connection::open(db_path)?;
    connection.busy_timeout(Duration::from_secs(5))?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    transaction.execute(
        "INSERT OR IGNORE INTO stripe_events (id, event_type, payload, received_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![id, event_type, payload, Utc::now().to_rfc3339()],
    )?;
    let processed: Option<String> = transaction
        .query_row(
            "SELECT processed_at FROM stripe_events WHERE id = ?1",
            [id],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    if processed.is_some() {
        transaction.commit()?;
        return Ok(None);
    }

    let event: serde_json::Value = serde_json::from_str(payload)?;
    let object = &event["data"]["object"];
    let notice = match event_type {
        "checkout.session.completed" if object["payment_status"].as_str() == Some("paid") => {
            let email = object["customer_details"]["email"]
                .as_str()
                .or_else(|| object["customer_email"].as_str())
                .map(str::to_lowercase)
                .context("Stripe-Checkout enthält keine E-Mail-Adresse")?;
            let offer = object["metadata"]["offer"]
                .as_str()
                .unwrap_or_default()
                .to_owned();
            let customer_id = object["customer"].as_str().unwrap_or_default();
            let subscription_id = object["subscription"].as_str().unwrap_or_default();
            transaction.execute(
                "INSERT INTO billing_accounts
                 (email, stripe_customer_id, stripe_subscription_id, pro_active, single_credits, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(email) DO UPDATE SET
                   stripe_customer_id = CASE WHEN excluded.stripe_customer_id != '' THEN excluded.stripe_customer_id ELSE billing_accounts.stripe_customer_id END,
                   stripe_subscription_id = CASE WHEN excluded.stripe_subscription_id != '' THEN excluded.stripe_subscription_id ELSE billing_accounts.stripe_subscription_id END,
                   pro_active = MAX(billing_accounts.pro_active, excluded.pro_active),
                   single_credits = billing_accounts.single_credits + excluded.single_credits,
                   updated_at = excluded.updated_at",
                params![
                    email,
                    customer_id,
                    subscription_id,
                    offer == "pro",
                    i64::from(offer == "single"),
                    Utc::now().to_rfc3339()
                ],
            )?;
            matches!(offer.as_str(), "single" | "pro").then_some(PurchaseNotice { email, offer })
        }
        "invoice.paid" => {
            if let Some(customer_id) = object["customer"].as_str() {
                transaction.execute(
                    "UPDATE billing_accounts SET pro_active = 1, updated_at = ?1 WHERE stripe_customer_id = ?2",
                    params![Utc::now().to_rfc3339(), customer_id],
                )?;
            }
            None
        }
        "invoice.payment_failed" | "customer.subscription.deleted" => {
            if let Some(customer_id) = object["customer"].as_str() {
                transaction.execute(
                    "UPDATE billing_accounts SET pro_active = 0, updated_at = ?1 WHERE stripe_customer_id = ?2",
                    params![Utc::now().to_rfc3339(), customer_id],
                )?;
            }
            None
        }
        _ => None,
    };
    transaction.execute(
        "UPDATE stripe_events SET processed_at = ?1 WHERE id = ?2",
        params![Utc::now().to_rfc3339(), id],
    )?;
    transaction.commit()?;
    Ok(notice)
}

async fn imprint(State(state): State<Arc<AppState>>) -> Result<Json<ImprintResponse>, ApiError> {
    if let Some(cached) = state.imprint_cache.read().await.as_ref() {
        if cached.cached_at.elapsed() < IMPRINT_CACHE_TTL {
            return Ok(Json(ImprintResponse {
                sections: cached.sections.clone(),
                source_url: IMPRINT_URL,
            }));
        }
    }

    let fetched = state.http_client.get(IMPRINT_URL).send().await;
    let sections = match fetched {
        Ok(response) if response.status().is_success() => {
            let body = response.text().await.map_err(|error| {
                error!(%error, "imprint response could not be read");
                ApiError::upstream("Das Impressum konnte vorübergehend nicht geladen werden.")
            })?;
            extract_imprint_sections(&body).ok_or_else(|| {
                error!("imprint structure was not recognized");
                ApiError::upstream("Das Impressum konnte vorübergehend nicht geladen werden.")
            })?
        }
        Ok(response) => {
            error!(status = %response.status(), "imprint request failed");
            stale_imprint(&state).await?
        }
        Err(error) => {
            error!(%error, "imprint request failed");
            stale_imprint(&state).await?
        }
    };

    *state.imprint_cache.write().await = Some(CachedImprint {
        sections: sections.clone(),
        cached_at: Instant::now(),
    });
    Ok(Json(ImprintResponse {
        sections,
        source_url: IMPRINT_URL,
    }))
}

async fn stale_imprint(state: &AppState) -> Result<Vec<ImprintSection>, ApiError> {
    state
        .imprint_cache
        .read()
        .await
        .as_ref()
        .map(|cached| cached.sections.clone())
        .ok_or_else(|| {
            ApiError::upstream("Das Impressum konnte vorübergehend nicht geladen werden.")
        })
}

fn extract_imprint_sections(body: &str) -> Option<Vec<ImprintSection>> {
    let document = Html::parse_document(body);
    let block_selector = Selector::parse(".thrv_text_element").ok()?;
    let heading_selector = Selector::parse("h3").ok()?;
    let mut sections = Vec::new();
    let mut started = false;

    for block in document.select(&block_selector) {
        let Some(heading_node) = block.select(&heading_selector).next() else {
            continue;
        };
        let heading = normalized_text(heading_node.text());
        if heading.contains("Seitenbetreiber") {
            started = true;
        }
        if !started {
            continue;
        }
        if heading.eq_ignore_ascii_case("unsere Unternehmensbereiche") {
            break;
        }

        let heading_text = normalized_text(heading_node.text());
        let mut values = block
            .text()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        if values.first().is_some_and(|value| *value == heading_text) {
            values.remove(0);
        }
        if !heading_text.is_empty() && !values.is_empty() {
            sections.push(ImprintSection {
                heading: heading_text,
                lines: values,
            });
        }
    }

    (sections.len() >= 3).then_some(sections)
}

fn normalized_text<'a>(parts: impl Iterator<Item = &'a str>) -> String {
    parts
        .flat_map(str::split_whitespace)
        .collect::<Vec<_>>()
        .join(" ")
}

async fn gaeb_to_pdf(
    State(state): State<Arc<AppState>>,
    mut multipart: Multipart,
) -> Result<Response, ApiError> {
    let mut filename = "leistungsverzeichnis.x83".to_owned();
    let mut gaeb = Vec::new();
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|_| ApiError::bad_request("GAEB-Upload konnte nicht gelesen werden."))?
    {
        if field.name() == Some("gaeb") {
            filename = field.file_name().unwrap_or(&filename).to_owned();
            gaeb = field
                .bytes()
                .await
                .map_err(|_| ApiError::bad_request("GAEB-Datei konnte nicht gelesen werden."))?
                .to_vec();
        }
    }
    if gaeb.is_empty() {
        return Err(ApiError::bad_request("Bitte eine GAEB-Datei auswählen."));
    }
    if gaeb.len() > state.max_upload_bytes {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Die GAEB-Datei ist größer als 2 MB.",
        ));
    }
    let input_name = safe_filename(&filename);
    let output_name = format!(
        "{}.pdf",
        safe_filename(
            Path::new(&filename)
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or("leistungsverzeichnis")
        )
    );
    let bytes = task::spawn_blocking(move || -> Result<Vec<u8>> {
        let directory = tempfile::tempdir()?;
        let input = directory.path().join(input_name);
        let output = directory.path().join("leistungsverzeichnis.pdf");
        std::fs::write(&input, gaeb)?;
        let document = read_gaeb_xml(&input)?;
        write_gaeb_pdf(&document, &output)?;
        Ok(std::fs::read(output)?)
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(|error| ApiError::bad_request(format!("GAEB-Datei nicht lesbar: {error}")))?;

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/pdf"),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{output_name}\""))
            .map_err(|_| ApiError::internal())?,
    );
    Ok((headers, Body::from(bytes)).into_response())
}

async fn create_job(
    State(state): State<Arc<AppState>>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<impl IntoResponse, ApiError> {
    let mut form = UploadForm::default();
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|_| ApiError::bad_request("Upload konnte nicht gelesen werden."))?
    {
        let name = field.name().unwrap_or_default().to_owned();
        if name == "pdf" {
            form.filename = field
                .file_name()
                .unwrap_or("leistungsverzeichnis.pdf")
                .to_owned();
            form.pdf = field
                .bytes()
                .await
                .map_err(|_| ApiError::bad_request("PDF konnte nicht gelesen werden."))?
                .to_vec();
        } else {
            let value = field
                .text()
                .await
                .map_err(|_| ApiError::bad_request("Formularfeld konnte nicht gelesen werden."))?;
            match name.as_str() {
                "email" => form.email = value.trim().to_lowercase(),
                "contact_name" => form.contact_name = value.trim().to_owned(),
                "company" => form.company = value.trim().to_owned(),
                "phone" => form.phone = value.trim().to_owned(),
                "consent" => form.consent = value == "true" || value == "on",
                "email_fallback_consent" => {
                    form.email_fallback_consent = value == "true" || value == "on"
                }
                "feature_updates_consent" => {
                    form.feature_updates_consent = value == "true" || value == "on"
                }
                _ => {}
            }
        }
    }

    let access = billing_access_from_headers(&state, &headers).map_err(|_| ApiError::internal())?;
    let has_paid_access = access.is_some();
    validate_form(&form, &state, access.as_ref())?;
    let id = Uuid::new_v4().simple().to_string();
    let token = Uuid::new_v4().simple().to_string();
    let job_dir = state.data_dir.join("jobs").join(&id);
    fs::create_dir_all(&job_dir)
        .await
        .map_err(|_| ApiError::internal())?;
    let input_path = job_dir.join("input.pdf");
    fs::write(&input_path, &form.pdf)
        .await
        .map_err(|_| ApiError::internal())?;

    let expires_at = Utc::now() + ChronoDuration::hours(state.retention_hours);
    let db_state = state.clone();
    let db_form = form;
    let db_id = id.clone();
    let db_token = token.clone();
    let reserved = task::spawn_blocking(move || {
        reserve_job(
            &db_state,
            &db_id,
            &db_token,
            &db_form,
            &expires_at.to_rfc3339(),
            access.as_ref(),
        )
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(|_| ApiError::internal())?;
    if !reserved {
        let _ = fs::remove_dir_all(&job_dir).await;
        let message = if has_paid_access {
            "Kein aktives Pro-Abo oder Einzel-Credit verfügbar.".to_owned()
        } else {
            "Das kostenlose Monatslimit von 3 Dokumenten ist erreicht.".to_owned()
        };
        return Err(ApiError::new(StatusCode::TOO_MANY_REQUESTS, message));
    }

    let worker_state = state.clone();
    let worker_id = id.clone();
    tokio::spawn(async move {
        if let Err(err) = process_job(worker_state.clone(), worker_id.clone()).await {
            error!(job_id = %worker_id, error = %err, "conversion failed");
            let _ = set_job_failed(&worker_state, &worker_id);
        }
    });

    Ok((
        StatusCode::ACCEPTED,
        Json(CreateJobResponse {
            id,
            token,
            status: "queued",
        }),
    ))
}

async fn job_status(
    State(state): State<Arc<AppState>>,
    AxumPath(id): AxumPath<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Result<Json<JobStatusResponse>, ApiError> {
    let token = query
        .get("token")
        .ok_or_else(|| ApiError::not_found("Auftrag nicht gefunden."))?
        .to_owned();
    let db_state = state.clone();
    let record = task::spawn_blocking(move || load_job(&db_state, &id, &token))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?
        .ok_or_else(|| ApiError::not_found("Auftrag nicht gefunden."))?;
    let download_url =
        (record.status == "ready").then(|| format!("/download/{}/{}", record.id, record.token));
    Ok(Json(JobStatusResponse {
        id: record.id,
        status: record.status,
        filename: record.filename,
        error: record.error,
        download_url,
        expires_at: record.expires_at,
    }))
}

async fn download(
    State(state): State<Arc<AppState>>,
    AxumPath((id, token)): AxumPath<(String, String)>,
) -> Result<Response, ApiError> {
    let db_state = state.clone();
    let record = task::spawn_blocking(move || load_job(&db_state, &id, &token))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?
        .filter(|job| job.status == "ready")
        .ok_or_else(|| ApiError::not_found("Download nicht gefunden oder abgelaufen."))?;
    let path = state
        .data_dir
        .join("jobs")
        .join(&record.id)
        .join("output.x83");
    let bytes = fs::read(path)
        .await
        .map_err(|_| ApiError::not_found("Download nicht gefunden oder abgelaufen."))?;
    let filename = output_filename(&record.filename);
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/xml; charset=utf-8"),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_str(&format!("attachment; filename=\"{filename}\""))
            .map_err(|_| ApiError::internal())?,
    );
    Ok((headers, Body::from(bytes)).into_response())
}

async fn process_job(state: Arc<AppState>, id: String) -> Result<()> {
    set_job_status(&state, &id, "processing")?;
    let worker_state = state.clone();
    let worker_id = id.clone();
    let record =
        task::spawn_blocking(move || load_job_for_worker(&worker_state, &worker_id)).await??;
    let input = state.data_dir.join("jobs").join(&id).join("input.pdf");
    let output = state.data_dir.join("jobs").join(&id).join("output.x83");
    let report = state
        .data_dir
        .join("jobs")
        .join(&id)
        .join("fehlerprotokoll.txt");
    let processing_input = input.clone();
    let processing_output = output.clone();
    let processing_report = report.clone();
    let smtp = state.smtp.clone();
    let emailed_with_warnings = task::spawn_blocking(move || -> Result<bool> {
        let boq = parse_pdf(&processing_input)?;
        if record.billing_tier == "free" {
            let positions = count_positions(&boq.roots);
            if positions > 50 {
                anyhow::bail!(
                    "Die kostenlose Version unterstützt maximal 50 Positionen; erkannt wurden {positions}."
                );
            }
        }
        match write_x83(&boq, &processing_output, false) {
            Ok(()) => {
                inject_pdf_pngs(&processing_input, &processing_output, &boq)?;
                apply_provisional_flags(&processing_output, &boq)?;
                Ok(false)
            }
            Err(error) if record.email_fallback_consent => {
                write_x83(&boq, &processing_output, true)?;
                inject_pdf_pngs(&processing_input, &processing_output, &boq)?;
                apply_provisional_flags(&processing_output, &boq)?;
                let report_text = format!(
                    "PRÜFPFLICHTIGER X83-ENTWURF\n\
                     ===========================\n\n\
                     Der sichere Export wurde wegen folgender Konflikte blockiert.\n\
                     Die beigefügte X83 wurde fehlertolerant erzeugt und muss vor jeder\n\
                     weiteren Verwendung fachlich geprüft werden.\n\n{error}\n"
                );
                std::fs::write(&processing_report, &report_text)?;
                let smtp = smtp.context(
                    "E-Mail-Versand ist nicht konfiguriert. Bitte SMTP_* Variablen setzen.",
                )?;
                send_fallback_email(
                    &smtp,
                    &record,
                    &processing_input,
                    &processing_output,
                    &processing_report,
                )?;
                Ok(true)
            }
            Err(error) => Err(error),
        }
    })
    .await??;
    if emailed_with_warnings {
        set_job_emailed_with_warnings(&state, &id, state.diagnostic_retention_days)?;
    } else {
        set_job_status(&state, &id, "ready")?;
    }
    Ok(())
}

fn validate_form(
    form: &UploadForm,
    state: &AppState,
    access: Option<&BillingAccess>,
) -> Result<(), ApiError> {
    if form.contact_name.chars().count() < 2 {
        return Err(ApiError::bad_request("Bitte einen Kontaktnamen angeben."));
    }
    if !form.email.contains('@') || form.email.len() > 254 {
        return Err(ApiError::bad_request(
            "Bitte eine gültige E-Mail-Adresse angeben.",
        ));
    }
    if !form.consent {
        return Err(ApiError::bad_request(
            "Bitte die Datenschutzerklärung bestätigen und die Konvertierung beauftragen.",
        ));
    }
    if form.pdf.is_empty() {
        return Err(ApiError::bad_request("Bitte eine PDF-Datei auswählen."));
    }
    if let Some(access) = access {
        if access.email != form.email {
            return Err(ApiError::new(
                StatusCode::FORBIDDEN,
                "Bitte die beim Kauf verwendete E-Mail-Adresse verwenden.",
            ));
        }
    }
    let allowed_bytes = if access.is_some() {
        state.paid_max_upload_bytes
    } else {
        state.max_upload_bytes
    };
    if form.pdf.len() > allowed_bytes {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "Diese Zugangsart akzeptiert maximal {} MB.",
                allowed_bytes / 1024 / 1024
            ),
        ));
    }
    if !form.pdf.starts_with(b"%PDF-") {
        return Err(ApiError::bad_request("Die Datei ist keine gültige PDF."));
    }
    Ok(())
}

fn init_database(state: &AppState) -> Result<()> {
    let connection = Connection::open(&state.db_path)?;
    connection.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA busy_timeout=5000;
         CREATE TABLE IF NOT EXISTS jobs (
           id TEXT PRIMARY KEY,
           token TEXT NOT NULL,
           email TEXT NOT NULL,
           contact_name TEXT NOT NULL,
           company TEXT NOT NULL,
           phone TEXT NOT NULL,
           filename TEXT NOT NULL,
           status TEXT NOT NULL,
           error TEXT,
           email_fallback_consent INTEGER NOT NULL DEFAULT 0,
           feature_updates_consent INTEGER NOT NULL DEFAULT 0,
           email_fallback_consent_at TEXT,
           feature_updates_consent_at TEXT,
           created_at TEXT NOT NULL,
           expires_at TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS jobs_email_created ON jobs(email, created_at);
         CREATE TABLE IF NOT EXISTS feature_subscriptions (
           email TEXT PRIMARY KEY,
           contact_name TEXT NOT NULL,
           consent_at TEXT NOT NULL,
           source TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS stripe_events (
           id TEXT PRIMARY KEY,
           event_type TEXT NOT NULL,
           payload TEXT NOT NULL,
           received_at TEXT NOT NULL,
           processed_at TEXT
         );
         CREATE TABLE IF NOT EXISTS billing_accounts (
           email TEXT PRIMARY KEY,
           stripe_customer_id TEXT NOT NULL DEFAULT '',
           stripe_subscription_id TEXT NOT NULL DEFAULT '',
           pro_active INTEGER NOT NULL DEFAULT 0,
           single_credits INTEGER NOT NULL DEFAULT 0,
           updated_at TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS billing_access_tokens (
           token_hash TEXT PRIMARY KEY,
           email TEXT NOT NULL,
           expires_at TEXT NOT NULL,
           created_at TEXT NOT NULL
         );
         CREATE TABLE IF NOT EXISTS billing_sessions (
           session_hash TEXT PRIMARY KEY,
           email TEXT NOT NULL,
           expires_at TEXT NOT NULL,
           created_at TEXT NOT NULL
         );",
    )?;
    ensure_job_column(
        &connection,
        "email_fallback_consent",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_job_column(
        &connection,
        "feature_updates_consent",
        "INTEGER NOT NULL DEFAULT 0",
    )?;
    ensure_job_column(&connection, "email_fallback_consent_at", "TEXT")?;
    ensure_job_column(&connection, "feature_updates_consent_at", "TEXT")?;
    ensure_job_column(&connection, "billing_tier", "TEXT NOT NULL DEFAULT 'free'")?;
    ensure_table_column(&connection, "stripe_events", "processed_at", "TEXT")?;
    connection.execute(
        "UPDATE jobs
         SET status = 'failed',
             error = 'Die Verarbeitung wurde durch einen Server-Neustart unterbrochen. Bitte erneut hochladen.'
         WHERE status IN ('queued', 'processing')",
        [],
    )?;
    Ok(())
}

fn ensure_table_column(
    connection: &Connection,
    table: &str,
    name: &str,
    definition: &str,
) -> Result<()> {
    let mut statement = connection.prepare(&format!("PRAGMA table_info({table})"))?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !columns.iter().any(|column| column == name) {
        connection.execute(
            &format!("ALTER TABLE {table} ADD COLUMN {name} {definition}"),
            [],
        )?;
    }
    Ok(())
}

fn ensure_job_column(connection: &Connection, name: &str, definition: &str) -> Result<()> {
    let mut statement = connection.prepare("PRAGMA table_info(jobs)")?;
    let columns = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    if !columns.iter().any(|column| column == name) {
        connection.execute(
            &format!("ALTER TABLE jobs ADD COLUMN {name} {definition}"),
            [],
        )?;
    }
    Ok(())
}

fn reserve_job(
    state: &AppState,
    id: &str,
    token: &str,
    form: &UploadForm,
    expires_at: &str,
    access: Option<&BillingAccess>,
) -> Result<bool> {
    let mut connection = Connection::open(&state.db_path)?;
    connection.busy_timeout(Duration::from_secs(5))?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let billing_tier = if let Some(access) = access {
        let account: Option<(bool, i64)> = transaction
            .query_row(
                "SELECT pro_active, single_credits FROM billing_accounts WHERE email = ?1",
                [&access.email],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        match account {
            Some((true, _)) => "pro",
            Some((false, credits)) if credits > 0 => {
                transaction.execute(
                    "UPDATE billing_accounts SET single_credits = single_credits - 1, updated_at = ?1 WHERE email = ?2 AND single_credits > 0",
                    params![Utc::now().to_rfc3339(), access.email],
                )?;
                "single"
            }
            _ => return Ok(false),
        }
    } else {
        let used_this_month: u32 = transaction.query_row(
            "SELECT COUNT(*) FROM jobs
             WHERE email = ?1 AND strftime('%Y-%m', created_at) = strftime('%Y-%m', 'now')
               AND billing_tier = 'free'",
            [&form.email],
            |row| row.get(0),
        )?;
        if used_this_month >= 3 {
            return Ok(false);
        }
        "free"
    };
    let now = Utc::now().to_rfc3339();
    transaction.execute(
        "INSERT INTO jobs
         (id, token, email, contact_name, company, phone, filename, status,
          email_fallback_consent, feature_updates_consent,
          email_fallback_consent_at, feature_updates_consent_at, created_at, expires_at, billing_tier)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'queued', ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            id,
            token,
            form.email,
            form.contact_name,
            form.company,
            form.phone,
            safe_filename(&form.filename),
            form.email_fallback_consent,
            form.feature_updates_consent,
            form.email_fallback_consent.then_some(now.as_str()),
            form.feature_updates_consent.then_some(now.as_str()),
            now,
            expires_at,
            billing_tier,
        ],
    )?;
    if form.feature_updates_consent {
        transaction.execute(
            "INSERT INTO feature_subscriptions (email, contact_name, consent_at, source)
             VALUES (?1, ?2, ?3, 'gaeb-web')
             ON CONFLICT(email) DO UPDATE SET
               contact_name = excluded.contact_name,
               consent_at = excluded.consent_at,
               source = excluded.source",
            params![form.email, form.contact_name, Utc::now().to_rfc3339()],
        )?;
    }
    transaction.commit()?;
    Ok(true)
}

fn load_job(state: &AppState, id: &str, token: &str) -> Result<Option<JobRecord>> {
    let connection = Connection::open(&state.db_path)?;
    connection
        .query_row(
            "SELECT id, token, filename, status, error, expires_at,
                    email, contact_name, email_fallback_consent, billing_tier
             FROM jobs
             WHERE id = ?1 AND token = ?2 AND expires_at > ?3",
            params![id, token, Utc::now().to_rfc3339()],
            |row| {
                Ok(JobRecord {
                    id: row.get(0)?,
                    token: row.get(1)?,
                    filename: row.get(2)?,
                    status: row.get(3)?,
                    error: row.get(4)?,
                    expires_at: row.get(5)?,
                    email: row.get(6)?,
                    contact_name: row.get(7)?,
                    email_fallback_consent: row.get(8)?,
                    billing_tier: row.get(9)?,
                })
            },
        )
        .optional()
        .map_err(Into::into)
}

fn load_job_for_worker(state: &AppState, id: &str) -> Result<JobRecord> {
    let connection = Connection::open(&state.db_path)?;
    connection
        .query_row(
            "SELECT id, token, filename, status, error, expires_at,
                    email, contact_name, email_fallback_consent, billing_tier
             FROM jobs WHERE id = ?1",
            [id],
            |row| {
                Ok(JobRecord {
                    id: row.get(0)?,
                    token: row.get(1)?,
                    filename: row.get(2)?,
                    status: row.get(3)?,
                    error: row.get(4)?,
                    expires_at: row.get(5)?,
                    email: row.get(6)?,
                    contact_name: row.get(7)?,
                    email_fallback_consent: row.get(8)?,
                    billing_tier: row.get(9)?,
                })
            },
        )
        .context("Auftrag nicht gefunden")
}

fn set_job_status(state: &AppState, id: &str, status: &str) -> Result<()> {
    let connection = Connection::open(&state.db_path)?;
    connection.execute(
        "UPDATE jobs SET status = ?1, error = NULL WHERE id = ?2",
        params![status, id],
    )?;
    Ok(())
}

fn set_job_emailed_with_warnings(state: &AppState, id: &str, retention_days: i64) -> Result<()> {
    let connection = Connection::open(&state.db_path)?;
    let expires_at = (Utc::now() + ChronoDuration::days(retention_days)).to_rfc3339();
    connection.execute(
        "UPDATE jobs
         SET status = 'emailed_with_warnings',
             error = 'Prüfpflichtiger X83-Entwurf und Fehlerprotokoll wurden per E-Mail versendet.',
             expires_at = ?1
         WHERE id = ?2",
        params![expires_at, id],
    )?;
    Ok(())
}

fn set_job_failed(state: &AppState, id: &str) -> Result<()> {
    let mut connection = Connection::open(&state.db_path)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let refundable_email: Option<String> = transaction
        .query_row(
            "SELECT email FROM jobs WHERE id = ?1 AND billing_tier = 'single' AND status != 'failed'",
            [id],
            |row| row.get(0),
        )
        .optional()?;
    transaction.execute(
        "UPDATE jobs SET status = 'failed',
         error = 'Das LV konnte nicht sicher konvertiert werden. Bitte prüfen Sie das PDF.'
         WHERE id = ?1",
        [id],
    )?;
    if let Some(email) = refundable_email {
        transaction.execute(
            "UPDATE billing_accounts SET single_credits = single_credits + 1, updated_at = ?1 WHERE email = ?2",
            params![Utc::now().to_rfc3339(), email],
        )?;
    }
    transaction.commit()?;
    Ok(())
}

fn count_positions(nodes: &[gaeb_toolkit::model::Node]) -> usize {
    nodes
        .iter()
        .map(|node| node.positions.len() + count_positions(&node.children))
        .sum()
}

fn send_fallback_email(
    smtp: &SmtpConfig,
    record: &JobRecord,
    input: &Path,
    output: &Path,
    report: &Path,
) -> Result<()> {
    let pdf = std::fs::read(input)?;
    let x83 = std::fs::read(output)?;
    let log = std::fs::read(report)?;
    let x83_name = format!("PRUEFEN_{}", output_filename(&record.filename));
    let report_name = format!(
        "{}_Fehlerprotokoll.txt",
        safe_filename(
            Path::new(&record.filename)
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or("leistungsverzeichnis")
        )
    );
    let body = format!(
        "Guten Tag {},\n\n\
         der sichere X83-Export war wegen erkannter Konflikte nicht möglich.\n\
         Auf Ihren Wunsch erhalten Sie:\n\
         - das Original-PDF,\n\
         - einen ausdrücklich prüfpflichtigen X83-Entwurf und\n\
         - das Fehlerprotokoll.\n\n\
         Verwenden Sie den X83-Entwurf erst nach fachlicher Prüfung.\n\
         Die Diagnosedaten werden spätestens nach dem vereinbarten Zeitraum gelöscht.\n",
        record.contact_name
    );
    let mixed = MultiPart::mixed()
        .singlepart(
            SinglePart::builder()
                .header(ContentType::TEXT_PLAIN)
                .body(body),
        )
        .singlepart(
            Attachment::new(record.filename.clone())
                .body(pdf, ContentType::parse("application/pdf")?),
        )
        .singlepart(Attachment::new(x83_name).body(x83, ContentType::parse("application/xml")?))
        .singlepart(
            Attachment::new(report_name)
                .body(log, ContentType::parse("text/plain; charset=utf-8")?),
        );
    let message = Message::builder()
        .from(smtp.from.parse()?)
        .to(record.email.parse()?)
        .subject("GAEB-Konvertierung: prüfpflichtiger X83-Entwurf")
        .multipart(mixed)?;
    let mut builder = SmtpTransport::relay(&smtp.host)?.port(smtp.port);
    if !smtp.username.is_empty() {
        builder = builder.credentials(Credentials::new(
            smtp.username.clone(),
            smtp.password.clone(),
        ));
    }
    builder.build().send(&message)?;
    Ok(())
}

fn backfill_stripe_events(state: &AppState) -> Result<Vec<PurchaseNotice>> {
    let connection = Connection::open(&state.db_path)?;
    let events = {
        let mut statement = connection.prepare(
            "SELECT id, event_type, payload FROM stripe_events WHERE processed_at IS NULL ORDER BY received_at",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    drop(connection);
    let mut notices = Vec::new();
    for (id, event_type, payload) in events {
        if let Some(notice) =
            record_and_apply_stripe_event(&state.db_path, &id, &event_type, &payload)?
        {
            notices.push(notice);
        }
    }
    Ok(notices)
}

fn issue_billing_access_email(state: &AppState, notice: &PurchaseNotice) -> Result<()> {
    let smtp = state
        .smtp
        .as_ref()
        .context("SMTP ist für den Käufer-Zugangslink nicht konfiguriert")?;
    let stripe = state
        .stripe
        .as_ref()
        .context("PUBLIC_BASE_URL ist nicht konfiguriert")?;
    let raw_token = Uuid::new_v4().simple().to_string();
    let token_hash = hash_token(&raw_token);
    let expires_at = (Utc::now() + ChronoDuration::hours(24)).to_rfc3339();
    let connection = Connection::open(&state.db_path)?;
    connection.execute(
        "INSERT INTO billing_access_tokens (token_hash, email, expires_at, created_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            token_hash,
            notice.email,
            expires_at,
            Utc::now().to_rfc3339()
        ],
    )?;
    let link = format!("{}/billing/access/{}", stripe.public_base_url, raw_token);
    let product = if notice.offer == "pro" {
        "GAEB Pro"
    } else {
        "eine Einzelkonvertierung"
    };
    let body = format!(
        "Guten Tag,\n\nIhre Zahlung für {product} wurde bestätigt.\n\n\
         Öffnen Sie innerhalb von 24 Stunden diesen persönlichen Zugangslink:\n{link}\n\n\
         Anschließend ist Ihr gekauftes Kontingent in diesem Browser freigeschaltet.\n\n\
         Falls Sie den Kauf nicht durchgeführt haben, ignorieren Sie diese Nachricht.\n"
    );
    let message = Message::builder()
        .from(smtp.from.parse()?)
        .to(notice.email.parse()?)
        .subject("Ihr Zugang zum GAEB-Konverter")
        .header(ContentType::TEXT_PLAIN)
        .body(body)?;
    let mut builder = SmtpTransport::relay(&smtp.host)?.port(smtp.port);
    if !smtp.username.is_empty() {
        builder = builder.credentials(Credentials::new(
            smtp.username.clone(),
            smtp.password.clone(),
        ));
    }
    builder.build().send(&message)?;
    Ok(())
}

fn activate_access_token(db_path: &Path, token_hash: &str, session_hash: &str) -> Result<()> {
    let mut connection = Connection::open(db_path)?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let email: String = transaction.query_row(
        "SELECT email FROM billing_access_tokens WHERE token_hash = ?1 AND expires_at > ?2",
        params![token_hash, Utc::now().to_rfc3339()],
        |row| row.get(0),
    )?;
    transaction.execute(
        "INSERT INTO billing_sessions (session_hash, email, expires_at, created_at)
         VALUES (?1, ?2, ?3, ?4)",
        params![
            session_hash,
            email,
            (Utc::now() + ChronoDuration::days(30)).to_rfc3339(),
            Utc::now().to_rfc3339()
        ],
    )?;
    transaction.execute(
        "DELETE FROM billing_access_tokens WHERE token_hash = ?1",
        [token_hash],
    )?;
    transaction.commit()?;
    Ok(())
}

fn billing_access_from_headers(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<Option<BillingAccess>> {
    let Some(raw_session) = cookie_value(headers, "gaeb_session") else {
        return Ok(None);
    };
    let connection = Connection::open(&state.db_path)?;
    connection
        .query_row(
            "SELECT email FROM billing_sessions WHERE session_hash = ?1 AND expires_at > ?2",
            params![hash_token(&raw_session), Utc::now().to_rfc3339()],
            |row| Ok(BillingAccess { email: row.get(0)? }),
        )
        .optional()
        .map_err(Into::into)
}

fn cookie_value(headers: &HeaderMap, name: &str) -> Option<String> {
    headers
        .get(header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .filter_map(|part| part.trim().split_once('='))
        .find_map(|(key, value)| (key == name).then(|| value.to_owned()))
}

fn hash_token(value: &str) -> String {
    Sha256::digest(value.as_bytes())
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

async fn cleanup_expired_jobs(state: &AppState) -> Result<()> {
    let db_path = state.db_path.clone();
    let ids = task::spawn_blocking(move || -> Result<Vec<String>> {
        let connection = Connection::open(db_path)?;
        let now = Utc::now().to_rfc3339();
        let ids = {
            let mut statement = connection.prepare("SELECT id FROM jobs WHERE expires_at <= ?1")?;
            let rows = statement
                .query_map([&now], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            rows
        };
        connection.execute("DELETE FROM jobs WHERE expires_at <= ?1", [&now])?;
        Ok(ids)
    })
    .await??;
    for id in &ids {
        let path = state.data_dir.join("jobs").join(id);
        let _ = fs::remove_dir_all(path).await;
    }
    Ok(())
}

fn safe_filename(value: &str) -> String {
    let source = Path::new(value)
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("leistungsverzeichnis.pdf");
    let mut filename = String::with_capacity(source.len());
    for character in source.chars() {
        let replacement = match character {
            'ä' => "ae",
            'ö' => "oe",
            'ü' => "ue",
            'Ä' => "Ae",
            'Ö' => "Oe",
            'Ü' => "Ue",
            'ß' => "ss",
            _ if character.is_ascii_alphanumeric()
                || matches!(character, '.' | '-' | '_' | ' ') =>
            {
                filename.push(character);
                continue;
            }
            _ => continue,
        };
        filename.push_str(replacement);
        if filename.len() >= 160 {
            break;
        }
    }
    filename.truncate(filename.len().min(160));
    if filename.trim_matches(['.', ' ']).is_empty() {
        "leistungsverzeichnis.pdf".to_owned()
    } else {
        filename
    }
}

fn output_filename(input: &str) -> String {
    let stem = Path::new(input)
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("leistungsverzeichnis");
    format!("{}.x83", safe_filename(stem))
}

fn env_usize(name: &str, default: usize) -> usize {
    env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn env_i64(name: &str, default: i64) -> i64 {
    env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn valid_email(value: &str) -> bool {
    value.len() <= 254
        && value.split_once('@').is_some_and(|(local, domain)| {
            !local.is_empty()
                && domain.contains('.')
                && !domain.starts_with('.')
                && !domain.ends_with('.')
        })
}

fn stripe_config() -> Option<StripeConfig> {
    let secret_key = required_env("STRIPE_SECRET_KEY")?;
    let webhook_secret = required_env("STRIPE_WEBHOOK_SECRET")?;
    let single_price_id = required_env("STRIPE_SINGLE_PRICE_ID")?;
    let pro_price_id = required_env("STRIPE_PRO_PRICE_ID")?;
    let public_base_url = required_env("PUBLIC_BASE_URL")?;
    if !secret_key.starts_with("sk_")
        || !webhook_secret.starts_with("whsec_")
        || !single_price_id.starts_with("price_")
        || !pro_price_id.starts_with("price_")
        || !public_base_url.starts_with("https://")
    {
        error!("Stripe-Konfiguration ist unvollständig oder ungültig; Checkout bleibt deaktiviert");
        return None;
    }
    Some(StripeConfig {
        secret_key,
        webhook_secret,
        single_price_id,
        pro_price_id,
        public_base_url: public_base_url.trim_end_matches('/').to_owned(),
    })
}

fn required_env(name: &str) -> Option<String> {
    env::var(name)
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

fn smtp_config() -> Option<SmtpConfig> {
    let host = env::var("SMTP_HOST").ok()?.trim().to_owned();
    let from = env::var("SMTP_FROM").ok()?.trim().to_owned();
    if host.is_empty() || from.is_empty() {
        return None;
    }
    Some(SmtpConfig {
        host,
        port: env::var("SMTP_PORT")
            .ok()
            .and_then(|value| value.parse().ok())
            .unwrap_or(587),
        username: env::var("SMTP_USERNAME").unwrap_or_default(),
        password: env::var("SMTP_PASSWORD").unwrap_or_default(),
        from,
    })
}

fn tracking_config() -> PublicTrackingConfig {
    PublicTrackingConfig {
        google_tag_manager_id: env::var("GOOGLE_TAG_MANAGER_ID")
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| {
                value.starts_with("GTM-")
                    && value.len() <= 32
                    && value.chars().all(|character| {
                        character.is_ascii_uppercase()
                            || character.is_ascii_digit()
                            || character == '-'
                    })
            }),
        google_analytics_id: env::var("GOOGLE_ANALYTICS_ID")
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| {
                value.starts_with("G-")
                    && value.len() <= 32
                    && value.chars().all(|character| {
                        character.is_ascii_uppercase()
                            || character.is_ascii_digit()
                            || character == '-'
                    })
            }),
        meta_pixel_id: env::var("META_PIXEL_ID")
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| {
                !value.is_empty()
                    && value.len() <= 32
                    && value.chars().all(|character| character.is_ascii_digit())
            }),
        klicktipp_pixel_url: env::var("KLICKTIPP_PIXEL_URL")
            .ok()
            .map(|value| value.trim().to_owned())
            .filter(|value| {
                value.starts_with("https://")
                    && value.len() <= 2048
                    && !value.chars().any(char::is_whitespace)
            }),
        consent_version: env::var("COOKIE_CONSENT_VERSION")
            .unwrap_or_else(|_| "1".to_owned())
            .trim()
            .chars()
            .filter(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_'))
            .take(32)
            .collect::<String>(),
    }
}

struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }

    fn not_found(message: impl Into<String>) -> Self {
        Self::new(StatusCode::NOT_FOUND, message)
    }

    fn internal() -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Der Auftrag konnte nicht verarbeitet werden.",
        )
    }

    fn upstream(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_GATEWAY, message)
    }

    fn unavailable(message: impl Into<String>) -> Self {
        Self::new(StatusCode::SERVICE_UNAVAILABLE, message)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ErrorResponse {
                error: self.message,
            }),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::Utc;
    use hmac::{Hmac, Mac};
    use rusqlite::Connection;
    use sha2::Sha256;
    use tempfile::tempdir;

    use super::{
        extract_imprint_sections, init_database, output_filename, record_and_apply_stripe_event,
        verify_stripe_signature, AppState,
    };

    #[test]
    fn imprint_extraction_omits_page_title_and_following_company_area() {
        let html = r#"
          <h1>Impressum</h1>
          <div class="thrv_text_element"><h3>Seitenbetreiber/ Verantwortlicher</h3>Hawk Vision GmbH<br>Tulpenweg 2</div>
          <div class="thrv_text_element"><h3>Kontaktdaten</h3><p>office@example.de<br>Telefon: 123</p></div>
          <div class="thrv_text_element"><h3>Unternehmensangaben</h3><p>Amtsgericht Jena</p></div>
          <div class="thrv_text_element"><h3>Quellenangaben</h3><p>finden Sie hier</p></div>
          <div class="thrv_text_element"><h3>unsere Unternehmensbereiche</h3><p>Nicht übernehmen</p></div>
        "#;
        let sections = extract_imprint_sections(html).unwrap();
        assert_eq!(sections.len(), 4);
        assert_eq!(sections[0].heading, "Seitenbetreiber/ Verantwortlicher");
        assert!(sections
            .iter()
            .all(|section| section.heading != "Impressum"));
        assert!(sections.iter().all(|section| !section
            .lines
            .iter()
            .any(|line| line.contains("Nicht übernehmen"))));
    }

    #[test]
    fn output_filename_is_ascii_safe_for_http_headers() {
        assert_eq!(
            output_filename("Angebot Außenputz Prüffläche.pdf"),
            "Angebot Aussenputz Pruefflaeche.x83"
        );
    }

    #[test]
    fn stripe_signature_accepts_valid_payload_and_rejects_changes() {
        let timestamp = Utc::now().timestamp();
        let body = br#"{"id":"evt_test","type":"checkout.session.completed"}"#;
        let secret = "whsec_test_secret";
        let signed = format!("{timestamp}.{}", String::from_utf8_lossy(body));
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(signed.as_bytes());
        let signature = mac
            .finalize()
            .into_bytes()
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let header = format!("t={timestamp},v1={signature}");

        assert!(verify_stripe_signature(&header, body, secret).is_ok());
        assert!(verify_stripe_signature(&header, b"changed", secret).is_err());
    }

    #[test]
    fn database_migration_adds_consent_columns_to_existing_jobs() {
        let directory = tempdir().unwrap();
        let db_path = directory.path().join("gaeb-web.sqlite3");
        let connection = Connection::open(&db_path).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE jobs (
                   id TEXT PRIMARY KEY,
                   token TEXT NOT NULL,
                   email TEXT NOT NULL,
                   contact_name TEXT NOT NULL,
                   company TEXT NOT NULL,
                   phone TEXT NOT NULL,
                   filename TEXT NOT NULL,
                   status TEXT NOT NULL,
                   error TEXT,
                   created_at TEXT NOT NULL,
                   expires_at TEXT NOT NULL
                 );",
            )
            .unwrap();
        drop(connection);

        let state = Arc::new(AppState {
            data_dir: directory.path().to_owned(),
            db_path: db_path.clone(),
            max_upload_bytes: 2 * 1024 * 1024,
            paid_max_upload_bytes: 25 * 1024 * 1024,
            retention_hours: 24,
            diagnostic_retention_days: 30,
            smtp: None,
            http_client: reqwest::Client::new(),
            imprint_cache: Arc::new(tokio::sync::RwLock::new(None)),
            tracking: Default::default(),
            stripe: None,
        });
        init_database(&state).unwrap();

        let connection = Connection::open(db_path).unwrap();
        let mut statement = connection.prepare("PRAGMA table_info(jobs)").unwrap();
        let columns = statement
            .query_map([], |row| row.get::<_, String>(1))
            .unwrap()
            .collect::<rusqlite::Result<Vec<_>>>()
            .unwrap();
        assert!(columns.contains(&"email_fallback_consent".to_owned()));
        assert!(columns.contains(&"feature_updates_consent".to_owned()));
        assert!(columns.contains(&"email_fallback_consent_at".to_owned()));
        assert!(columns.contains(&"feature_updates_consent_at".to_owned()));
    }

    #[test]
    fn paid_checkout_grants_exactly_one_credit_even_when_repeated() {
        let directory = tempdir().unwrap();
        let db_path = directory.path().join("gaeb-web.sqlite3");
        let state = Arc::new(AppState {
            data_dir: directory.path().to_owned(),
            db_path: db_path.clone(),
            max_upload_bytes: 2 * 1024 * 1024,
            paid_max_upload_bytes: 25 * 1024 * 1024,
            retention_hours: 24,
            diagnostic_retention_days: 30,
            smtp: None,
            http_client: reqwest::Client::new(),
            imprint_cache: Arc::new(tokio::sync::RwLock::new(None)),
            tracking: Default::default(),
            stripe: None,
        });
        init_database(&state).unwrap();
        let payload = r#"{
          "id":"evt_paid_once",
          "type":"checkout.session.completed",
          "data":{"object":{
            "payment_status":"paid",
            "customer":"cus_test",
            "customer_details":{"email":"BUYER@EXAMPLE.DE"},
            "metadata":{"offer":"single"}
          }}
        }"#;

        let first = record_and_apply_stripe_event(
            &db_path,
            "evt_paid_once",
            "checkout.session.completed",
            payload,
        )
        .unwrap();
        let repeated = record_and_apply_stripe_event(
            &db_path,
            "evt_paid_once",
            "checkout.session.completed",
            payload,
        )
        .unwrap();
        let connection = Connection::open(db_path).unwrap();
        let credits: i64 = connection
            .query_row(
                "SELECT single_credits FROM billing_accounts WHERE email = 'buyer@example.de'",
                [],
                |row| row.get(0),
            )
            .unwrap();

        assert_eq!(first.unwrap().email, "buyer@example.de");
        assert!(repeated.is_none());
        assert_eq!(credits, 1);
    }
}

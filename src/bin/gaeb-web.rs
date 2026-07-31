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
    body::Body,
    extract::{DefaultBodyLimit, Multipart, Path as AxumPath, Query, State},
    http::{header, HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::{Duration as ChronoDuration, Utc};
use gaeb_toolkit::{
    apply_provisional_flags, inject_pdf_pngs, parse_pdf, read_gaeb_xml, write_gaeb_pdf, write_x83,
};
use lettre::{
    message::{header::ContentType, Attachment, MultiPart, SinglePart},
    transport::smtp::authentication::Credentials,
    Message, SmtpTransport, Transport,
};
use rusqlite::{params, Connection, OptionalExtension, TransactionBehavior};
use scraper::{Html, Selector};
use serde::Serialize;
use tokio::{fs, net::TcpListener, sync::RwLock, task};
use tower_http::{services::ServeDir, trace::TraceLayer};
use tracing::{error, info};
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

const DEFAULT_MAX_UPLOAD_BYTES: usize = 2 * 1024 * 1024;
const IMPRINT_URL: &str = "https://www.hawkvision.de/impressum/";
const IMPRINT_CACHE_TTL: Duration = Duration::from_secs(6 * 60 * 60);

#[derive(Clone)]
struct AppState {
    data_dir: PathBuf,
    db_path: PathBuf,
    daily_limit: u32,
    max_upload_bytes: usize,
    retention_hours: i64,
    diagnostic_retention_days: i64,
    smtp: Option<SmtpConfig>,
    http_client: reqwest::Client,
    imprint_cache: Arc<RwLock<Option<CachedImprint>>>,
    tracking: PublicTrackingConfig,
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
        daily_limit: env_u32("DAILY_LIMIT", 2),
        max_upload_bytes: env_usize("MAX_UPLOAD_BYTES", DEFAULT_MAX_UPLOAD_BYTES),
        retention_hours: env_i64("RETENTION_HOURS", 24),
        diagnostic_retention_days: env_i64("DIAGNOSTIC_RETENTION_DAYS", 30),
        smtp: smtp_config(),
        http_client: reqwest::Client::builder()
            .user_agent("GAEB-Konverter/0.1 (+https://gaeb.hawk-vision.de)")
            .timeout(Duration::from_secs(12))
            .build()?,
        imprint_cache: Arc::new(RwLock::new(None)),
        tracking: tracking_config(),
    });
    init_database(&state)?;

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
        .route("/api/jobs/{id}", get(job_status))
        .route("/download/{id}/{token}", get(download))
        .fallback_service(ServeDir::new("web").append_index_html_on_directories(true))
        .layer(DefaultBodyLimit::max(state.max_upload_bytes + 256 * 1024))
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

    validate_form(&form, &state)?;
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
        )
    })
    .await
    .map_err(|_| ApiError::internal())?
    .map_err(|_| ApiError::internal())?;
    if !reserved {
        let _ = fs::remove_dir_all(&job_dir).await;
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            format!(
                "Das kostenlose Tageslimit von {} Konvertierungen ist erreicht.",
                state.daily_limit
            ),
        ));
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

fn validate_form(form: &UploadForm, state: &AppState) -> Result<(), ApiError> {
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
            "Die Zustimmung zur Verarbeitung ist erforderlich.",
        ));
    }
    if form.pdf.is_empty() {
        return Err(ApiError::bad_request("Bitte eine PDF-Datei auswählen."));
    }
    if form.pdf.len() > state.max_upload_bytes {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            format!(
                "Die kostenlose Version akzeptiert maximal {} MB.",
                state.max_upload_bytes / 1024 / 1024
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
    connection.execute(
        "UPDATE jobs
         SET status = 'failed',
             error = 'Die Verarbeitung wurde durch einen Server-Neustart unterbrochen. Bitte erneut hochladen.'
         WHERE status IN ('queued', 'processing')",
        [],
    )?;
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
) -> Result<bool> {
    let mut connection = Connection::open(&state.db_path)?;
    connection.busy_timeout(Duration::from_secs(5))?;
    let transaction = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
    let used_today: u32 = transaction.query_row(
        "SELECT COUNT(*) FROM jobs
         WHERE email = ?1 AND date(created_at) = date('now')",
        [&form.email],
        |row| row.get(0),
    )?;
    if used_today >= state.daily_limit {
        return Ok(false);
    }
    let now = Utc::now().to_rfc3339();
    transaction.execute(
        "INSERT INTO jobs
         (id, token, email, contact_name, company, phone, filename, status,
          email_fallback_consent, feature_updates_consent,
          email_fallback_consent_at, feature_updates_consent_at, created_at, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'queued', ?8, ?9, ?10, ?11, ?12, ?13)",
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
                    email, contact_name, email_fallback_consent
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
                    email, contact_name, email_fallback_consent
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
    let connection = Connection::open(&state.db_path)?;
    connection.execute(
        "UPDATE jobs SET status = 'failed',
         error = 'Das LV konnte nicht sicher konvertiert werden. Bitte prüfen Sie das PDF.'
         WHERE id = ?1",
        [id],
    )?;
    Ok(())
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

fn env_u32(name: &str, default: u32) -> u32 {
    env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
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

    use rusqlite::Connection;
    use tempfile::tempdir;

    use super::{extract_imprint_sections, init_database, output_filename, AppState};

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
            daily_limit: 2,
            max_upload_bytes: 2 * 1024 * 1024,
            retention_hours: 24,
            diagnostic_retention_days: 30,
            smtp: None,
            http_client: reqwest::Client::new(),
            imprint_cache: Arc::new(tokio::sync::RwLock::new(None)),
            tracking: Default::default(),
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
}

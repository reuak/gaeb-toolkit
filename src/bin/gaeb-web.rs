use std::{
    collections::HashMap,
    env,
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
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
use gaeb_toolkit::{apply_provisional_flags, inject_pdf_pngs, parse_pdf, write_x83};
use rusqlite::{params, Connection, OptionalExtension};
use serde::Serialize;
use tokio::{fs, net::TcpListener, task};
use tower_http::{services::ServeDir, trace::TraceLayer};
use tracing::{error, info};
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

const DEFAULT_MAX_UPLOAD_BYTES: usize = 2 * 1024 * 1024;

#[derive(Clone)]
struct AppState {
    data_dir: PathBuf,
    db_path: PathBuf,
    daily_limit: u32,
    max_upload_bytes: usize,
    retention_hours: i64,
}

#[derive(Default)]
struct UploadForm {
    email: String,
    contact_name: String,
    company: String,
    phone: String,
    consent: bool,
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

struct JobRecord {
    id: String,
    token: String,
    filename: String,
    status: String,
    error: Option<String>,
    expires_at: String,
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
                _ => {}
            }
        }
    }

    validate_form(&form, &state)?;
    let state_for_limit = state.clone();
    let email_for_limit = form.email.clone();
    let used_today = task::spawn_blocking(move || daily_usage(&state_for_limit, &email_for_limit))
        .await
        .map_err(|_| ApiError::internal())?
        .map_err(|_| ApiError::internal())?;
    if used_today >= state.daily_limit {
        return Err(ApiError::new(
            StatusCode::TOO_MANY_REQUESTS,
            format!(
                "Das kostenlose Tageslimit von {} Konvertierungen ist erreicht.",
                state.daily_limit
            ),
        ));
    }

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
    task::spawn_blocking(move || {
        insert_job(
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
    let input = state.data_dir.join("jobs").join(&id).join("input.pdf");
    let output = state.data_dir.join("jobs").join(&id).join("output.x83");
    let processing_input = input.clone();
    let processing_output = output.clone();
    task::spawn_blocking(move || -> Result<()> {
        let boq = parse_pdf(&processing_input)?;
        write_x83(&boq, &processing_output, false)?;
        inject_pdf_pngs(&processing_input, &processing_output, &boq)?;
        apply_provisional_flags(&processing_output, &boq)?;
        Ok(())
    })
    .await??;
    set_job_status(&state, &id, "ready")?;
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
           created_at TEXT NOT NULL,
           expires_at TEXT NOT NULL
         );
         CREATE INDEX IF NOT EXISTS jobs_email_created ON jobs(email, created_at);",
    )?;
    Ok(())
}

fn insert_job(
    state: &AppState,
    id: &str,
    token: &str,
    form: &UploadForm,
    expires_at: &str,
) -> Result<()> {
    let connection = Connection::open(&state.db_path)?;
    connection.execute(
        "INSERT INTO jobs
         (id, token, email, contact_name, company, phone, filename, status, created_at, expires_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, 'queued', ?8, ?9)",
        params![
            id,
            token,
            form.email,
            form.contact_name,
            form.company,
            form.phone,
            safe_filename(&form.filename),
            Utc::now().to_rfc3339(),
            expires_at,
        ],
    )?;
    Ok(())
}

fn daily_usage(state: &AppState, email: &str) -> Result<u32> {
    let connection = Connection::open(&state.db_path)?;
    let count = connection.query_row(
        "SELECT COUNT(*) FROM jobs
         WHERE email = ?1 AND date(created_at) = date('now')",
        [email],
        |row| row.get(0),
    )?;
    Ok(count)
}

fn load_job(state: &AppState, id: &str, token: &str) -> Result<Option<JobRecord>> {
    let connection = Connection::open(&state.db_path)?;
    connection
        .query_row(
            "SELECT id, token, filename, status, error, expires_at
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
                })
            },
        )
        .optional()
        .map_err(Into::into)
}

fn set_job_status(state: &AppState, id: &str, status: &str) -> Result<()> {
    let connection = Connection::open(&state.db_path)?;
    connection.execute(
        "UPDATE jobs SET status = ?1, error = NULL WHERE id = ?2",
        params![status, id],
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
    use super::output_filename;

    #[test]
    fn output_filename_is_ascii_safe_for_http_headers() {
        assert_eq!(
            output_filename("Angebot Außenputz Prüffläche.pdf"),
            "Angebot Aussenputz Pruefflaeche.x83"
        );
    }
}

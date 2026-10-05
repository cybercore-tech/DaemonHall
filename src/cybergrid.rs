//! CYBERGRID HTTP adapter over Cybercore's shared theme document catalog.
//!
//! DaemonHall serves the same built-in and user-created themes as Cyberdeck
//! Hub, Cyberdesk, and Cybercore Theme Studio.

use axum::Json;
use axum::extract::{Path, Query};
use axum::http::{StatusCode, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use futures_util::stream::{self, Stream};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{convert::Infallible, time::Duration};

pub async fn theme_events() -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let events = stream::unfold(None, |last_revision| async move {
        loop {
            if let Ok(catalog) = catalog() {
                let revision = catalog.revision();
                if last_revision != Some(revision) {
                    let event = Event::default()
                        .event("theme-change")
                        .data(revision.to_string());
                    return Some((Ok(event), Some(revision)));
                }
            }
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    });
    Sse::new(events).keep_alive(KeepAlive::default())
}

fn catalog() -> Result<cybercore::theme::ThemeCatalog, cybercore::theme::ThemeError> {
    cybercore::theme::ThemeCatalog::load()
}

fn error(status: StatusCode, message: impl ToString) -> Response {
    (status, Json(json!({ "error": message.to_string() }))).into_response()
}

fn palette_json(palette: &cybercore::schema::Palette) -> Value {
    json!({
        "bg": palette.bg,
        "white": palette.white,
        "acid_green": palette.acid_green,
        "hot_pink": palette.hot_pink,
        "purple": palette.purple,
        "cyan": palette.cyan,
        "orange": palette.orange,
        "red": palette.red,
        "panel": palette.panel,
        "line": palette.line,
        "muted": palette.muted,
    })
}

pub async fn list_themes() -> Response {
    let Ok(catalog) = catalog() else {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not load theme catalog",
        );
    };
    let themes: Vec<Value> = catalog
        .iter()
        .map(|(id, entry)| {
            let document = &entry.document;
            json!({
                "id": id,
                "name": document.metadata.name,
                "family": document.metadata.family,
                "description": document.metadata.description,
                "author": document.metadata.author,
                "builtin": entry.builtin,
                "palette": palette_json(&document.palette),
                "variants": document.variants,
                "design": document.design,
            })
        })
        .collect();
    Json(json!({
        "active": catalog.active_id(),
        "appearance": catalog.active_appearance(),
        "themes": themes
    }))
    .into_response()
}

#[derive(Deserialize)]
pub struct CssQuery {
    appearance: Option<cybercore::theme::Appearance>,
}

pub async fn theme_css(Path(id): Path<String>, Query(query): Query<CssQuery>) -> Response {
    let Ok(catalog) = catalog() else {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "could not load theme catalog",
        );
    };
    let Some(entry) = catalog.get(&id) else {
        return error(StatusCode::NOT_FOUND, "theme not found");
    };
    (
        [(header::CONTENT_TYPE, "text/css; charset=utf-8")],
        entry
            .document
            .to_css(query.appearance.unwrap_or(catalog.active_appearance())),
    )
        .into_response()
}

pub async fn select_theme(Path(id): Path<String>) -> Response {
    let mut catalog = match catalog() {
        Ok(catalog) => catalog,
        Err(error) => return self::error(StatusCode::BAD_REQUEST, error),
    };
    match catalog.select(&id) {
        Ok(()) => Json(json!({ "active": id })).into_response(),
        Err(error) => self::error(StatusCode::BAD_REQUEST, error),
    }
}

pub async fn select_appearance(Path(mode): Path<String>) -> Response {
    let appearance = match mode.as_str() {
        "dark" => cybercore::theme::Appearance::Dark,
        "light" => cybercore::theme::Appearance::Light,
        _ => return error(StatusCode::BAD_REQUEST, "appearance must be dark or light"),
    };
    let mut catalog = match catalog() {
        Ok(catalog) => catalog,
        Err(error) => return self::error(StatusCode::BAD_REQUEST, error),
    };
    match catalog.set_appearance(appearance) {
        Ok(()) => Json(json!({ "appearance": appearance })).into_response(),
        Err(error) => self::error(StatusCode::BAD_REQUEST, error),
    }
}

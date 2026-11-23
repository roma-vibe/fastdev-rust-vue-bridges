//! Serves the built single-page app with history fallback to `index.html`.

use axum::{
    extract::Request,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use tower::ServiceExt as _;
use tower_http::services::{ServeDir, ServeFile};

use app_core::Ctx;

pub async fn serve(ctx: &Ctx, req: Request) -> Response {
    let dist = &ctx.config.web_dist;
    let index = dist.join("index.html");
    if !index.exists() {
        return (
            StatusCode::NOT_FOUND,
            "The web app is not built. Run `npm run build`, or open the Vite dev server (WEB_PORT) in development.",
        )
            .into_response();
    }
    let service = ServeDir::new(dist).fallback(ServeFile::new(index));
    match service.oneshot(req).await {
        Ok(resp) => resp.into_response(),
        // Missing files fall back to index.html, I/O errors become 500 inside ServeDir.
        Err(never) => match never {},
    }
}

//! `/api/auth/*`: registration, login and logout. Every login creates a new session (a fresh
//! random token) and sets its cookie (`app_core::auth::session_cookie`); logout deletes it. Both
//! sign-in and registration are throttled (`throttle.rs`).

use std::sync::Arc;

use app_core::{
    ApiError, ApiResult, Ctx, Json,
    auth::{
        check_origin, clear_session_cookie, create_session, delete_session, request_token,
        session_cookie,
    },
    settings,
};
use axum::{
    Extension,
    extract::State,
    http::{HeaderMap, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
    password,
    throttle::{Attempt, Throttle},
    users,
};

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthConfig {
    registration_open: bool,
    has_users: bool,
}

/// `GET /api/auth/config` (public) → `{registrationOpen, hasUsers}`.
pub async fn config(State(ctx): State<Ctx>) -> ApiResult<Json<AuthConfig>> {
    let has_users = users::count(&ctx.db).await? > 0;
    let registration_open = !has_users || registration_open(&ctx).await?;
    Ok(Json(AuthConfig {
        registration_open,
        has_users,
    }))
}

async fn registration_open(ctx: &Ctx) -> sqlx::Result<bool> {
    settings::get_or(&ctx.db, "registration_open", true).await
}

#[derive(Deserialize)]
pub struct RegisterInput {
    email: String,
    password: String,
    #[serde(default)]
    name: String,
}

/// `POST /api/auth/register` `{email, password, name?}` → 201 `{user}` + session cookie.
/// The first user becomes `admin`; later ones need the `registration_open` setting.
/// 429 after [`crate::throttle::REGISTRATIONS_PER_HOUR`] registrations of one client.
pub async fn register(
    State(ctx): State<Ctx>,
    Extension(throttle): Extension<Arc<Throttle>>,
    headers: HeaderMap,
    Json(input): Json<RegisterInput>,
) -> ApiResult<Response> {
    throttle.register(&headers)?;
    let email = users::clean_email(&input.email)?;
    let name = users::clean_name(&input.name)?;
    password::check(&input.password)?;
    let first = users::count(&ctx.db).await? == 0;
    if !first && !registration_open(&ctx).await? {
        return Err(ApiError::forbidden("Registration is closed"));
    }
    let hash = password::hash(&input.password).await?;
    let role = if first { "admin" } else { "user" };
    let user = users::insert(&ctx.db, &email, &name, &hash, role).await?;
    signed_in(&ctx, &headers, StatusCode::CREATED, user).await
}

#[derive(Deserialize)]
pub struct LoginInput {
    email: String,
    password: String,
}

/// `POST /api/auth/login` `{email, password}` → `{user}` + session cookie; 401 on wrong credentials
/// (the same answer, after the same work, whether the email exists or not); 429 while throttled.
pub async fn login(
    State(ctx): State<Ctx>,
    Extension(throttle): Extension<Arc<Throttle>>,
    headers: HeaderMap,
    Json(input): Json<LoginInput>,
) -> ApiResult<Response> {
    let attempt = Attempt::new(&input.email, &headers);
    throttle.sign_in(&attempt)?;
    let email = input.email.trim().to_lowercase();
    let account = users::for_login(&ctx.db, &email).await?;
    let matches = match &account {
        Some((_, hash, _)) => password::verify(&input.password, hash).await,
        None => {
            password::verify_decoy(&input.password).await;
            false
        }
    };
    let Some((user, _, disabled)) = account.filter(|_| matches) else {
        throttle.failed(&attempt);
        return Err(ApiError::unauthorized("Wrong email or password"));
    };
    throttle.succeeded(&attempt);
    if disabled {
        return Err(ApiError::forbidden("This account is disabled"));
    }
    signed_in(&ctx, &headers, StatusCode::OK, user).await
}

/// `POST /api/auth/logout` → 204; deletes the current session and the cookie. A cookie logout
/// passes the origin check like every other cookie write.
pub async fn logout(State(ctx): State<Ctx>, headers: HeaderMap) -> ApiResult<Response> {
    check_origin(&Method::POST, &headers, &ctx.config)?;
    if let Some(token) = request_token(&headers, &ctx.config) {
        delete_session(&ctx.db, &token).await?;
    }
    let cookie = clear_session_cookie(&ctx.config);
    Ok((StatusCode::NO_CONTENT, [(header::SET_COOKIE, cookie)]).into_response())
}

async fn signed_in(
    ctx: &Ctx,
    headers: &HeaderMap,
    status: StatusCode,
    user: users::User,
) -> ApiResult<Response> {
    let agent = headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|v| v.chars().take(300).collect::<String>());
    let token = create_session(&ctx.db, &user.id, agent.as_deref()).await?;
    let cookie = session_cookie(&ctx.config, &token);
    Ok((
        status,
        [(header::SET_COOKIE, cookie)],
        Json(json!({ "user": user })),
    )
        .into_response())
}

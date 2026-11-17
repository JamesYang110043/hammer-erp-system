//! 幣別、匯率、稅別（docs/architecture.md §6.1、§8.6、§10「專屬端點」）。
//!
//! 全部需要登入（[`AuthUser`]）；權限在 `hammer_core` 檢查。
//! request / response 型別與錯誤見 [`hammer_shared::currency`]、[`hammer_shared::exchange_rate`]、
//! [`hammer_shared::tax_rate`]。
//!
//! | 端點 | 權限 | 成功回應 |
//! |---|---|---|
//! | `GET /api/v1/currencies` | 登入 | `200` `Currency[]` |
//! | `POST /api/v1/currencies` | `settings.manage` | `201` [`Currency`] |
//! | `PATCH /api/v1/currencies/{code}` | `settings.manage` | `200` [`Currency`] |
//! | `GET /api/v1/exchange-rates` | `record.read` | `200` `Page<ExchangeRate>` |
//! | `GET /api/v1/exchange-rates/effective` | `record.read` | `200` [`EffectiveRate`] |
//! | `POST /api/v1/exchange-rates` | `record.write` | `201` [`ExchangeRate`] |
//! | `PATCH /api/v1/exchange-rates/{id}` | `record.write` | `200` [`ExchangeRate`] |
//! | `DELETE /api/v1/exchange-rates/{id}?version=N` | `record.write` | `204` |
//! | `GET /api/v1/tax-rates` | 登入 | `200` `TaxRate[]` |

use axum::Json;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode};
use hammer_core::currencies::{self, CurrencyChanges, NewCurrencyInput};
use hammer_core::exchange_rates::{self, NewRateInput};
use hammer_core::tax_rates;
use hammer_shared::Id;
use hammer_shared::currency::{
    CreateCurrencyRequest, Currency, CurrencyListQuery, UpdateCurrencyRequest,
};
use hammer_shared::exchange_rate::{
    CreateExchangeRateRequest, DeleteExchangeRateQuery, EffectiveRate, EffectiveRateQuery,
    ExchangeRate, ExchangeRateQuery, UpdateExchangeRateRequest,
};
use hammer_shared::pagination::Page;
use hammer_shared::tax_rate::TaxRate;
use time::OffsetDateTime;

use super::auth::request_client_info;
use crate::{AppError, AppState, AuthUser};

/// `GET /api/v1/currencies`
pub async fn list_currencies(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(query): Query<CurrencyListQuery>,
) -> Result<Json<Vec<Currency>>, AppError> {
    let include_inactive = query.include_inactive.unwrap_or(false);
    Ok(Json(
        currencies::list_currencies(&state.pool, &auth, include_inactive).await?,
    ))
}

/// `POST /api/v1/currencies`
pub async fn create_currency(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Json(req): Json<CreateCurrencyRequest>,
) -> Result<(StatusCode, Json<Currency>), AppError> {
    let info = request_client_info(&headers);
    let currency = currencies::create_currency(
        &state.pool,
        &auth,
        &NewCurrencyInput {
            code: &req.code,
            name: &req.name,
            symbol: req.symbol.as_deref(),
            decimal_places: req.decimal_places,
        },
        info.as_deref(),
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(currency)))
}

/// `PATCH /api/v1/currencies/{code}`
pub async fn update_currency(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Path(code): Path<String>,
    Json(req): Json<UpdateCurrencyRequest>,
) -> Result<Json<Currency>, AppError> {
    let info = request_client_info(&headers);
    let currency = currencies::update_currency(
        &state.pool,
        &auth,
        &code,
        &CurrencyChanges {
            version: req.version,
            name: req.name.as_deref(),
            symbol: req.symbol.as_deref(),
            decimal_places: req.decimal_places,
            is_active: req.is_active,
        },
        info.as_deref(),
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok(Json(currency))
}

/// `GET /api/v1/exchange-rates`
pub async fn list_exchange_rates(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(query): Query<ExchangeRateQuery>,
) -> Result<Json<Page<ExchangeRate>>, AppError> {
    Ok(Json(
        exchange_rates::list_exchange_rates(&state.pool, &auth, &query).await?,
    ))
}

/// `GET /api/v1/exchange-rates/effective`
pub async fn effective_rate(
    State(state): State<AppState>,
    auth: AuthUser,
    Query(query): Query<EffectiveRateQuery>,
) -> Result<Json<EffectiveRate>, AppError> {
    Ok(Json(
        exchange_rates::effective_rate(&state.pool, &auth, &query.currency_code, query.date)
            .await?,
    ))
}

/// `POST /api/v1/exchange-rates`
pub async fn create_exchange_rate(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Json(req): Json<CreateExchangeRateRequest>,
) -> Result<(StatusCode, Json<ExchangeRate>), AppError> {
    let info = request_client_info(&headers);
    let rate = exchange_rates::create_exchange_rate(
        &state.pool,
        &auth,
        &NewRateInput {
            currency_code: &req.currency_code,
            rate_date: req.rate_date,
            rate: req.rate,
        },
        info.as_deref(),
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok((StatusCode::CREATED, Json(rate)))
}

/// `PATCH /api/v1/exchange-rates/{id}`
pub async fn update_exchange_rate(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Path(id): Path<Id>,
    Json(req): Json<UpdateExchangeRateRequest>,
) -> Result<Json<ExchangeRate>, AppError> {
    let info = request_client_info(&headers);
    let rate = exchange_rates::update_exchange_rate(
        &state.pool,
        &auth,
        id,
        req.version,
        req.rate,
        info.as_deref(),
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok(Json(rate))
}

/// `DELETE /api/v1/exchange-rates/{id}?version=N`
pub async fn delete_exchange_rate(
    State(state): State<AppState>,
    headers: HeaderMap,
    auth: AuthUser,
    Path(id): Path<Id>,
    Query(query): Query<DeleteExchangeRateQuery>,
) -> Result<StatusCode, AppError> {
    let info = request_client_info(&headers);
    exchange_rates::delete_exchange_rate(
        &state.pool,
        &auth,
        id,
        query.version,
        info.as_deref(),
        OffsetDateTime::now_utc(),
    )
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `GET /api/v1/tax-rates`
pub async fn list_tax_rates(
    State(state): State<AppState>,
    auth: AuthUser,
) -> Result<Json<Vec<TaxRate>>, AppError> {
    Ok(Json(tax_rates::list_tax_rates(&state.pool, &auth).await?))
}

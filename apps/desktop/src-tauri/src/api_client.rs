//! API Server 的 HTTP client（`docs/architecture.md` §3.3）。
//!
//! - 每個請求自動帶 `X-Client-Version` 標頭（§10）
//! - 需要登入的請求帶 `Authorization: Bearer <token>`；伺服器回 `401` 時轉成
//!   [`ClientError::SessionExpired`]
//! - 伺服器回傳的錯誤轉成 shared 的 [`ApiError`]
//! - 上傳、下載附件的逾時為 [`TRANSFER_TIMEOUT`]（一般請求為 15 秒）

use std::io::Read;
use std::time::Duration;

use hammer_shared::attachment::{Attachment, UPLOAD_FIELD};
use hammer_shared::audit::{AuditLogEntry, AuditLogQuery};
use hammer_shared::auth::{ChangePasswordRequest, CurrentUser, LoginRequest, LoginResponse};
use hammer_shared::constants::{API_PREFIX, CLIENT_VERSION_HEADER};
use hammer_shared::currency::{
    CreateCurrencyRequest, Currency, CurrencyListQuery, UpdateCurrencyRequest,
};
use hammer_shared::document::{DocumentAction, DocumentActionRequest};
use hammer_shared::exchange_rate::{
    CreateExchangeRateRequest, DeleteExchangeRateQuery, EffectiveRate, EffectiveRateQuery,
    ExchangeRate, ExchangeRateQuery, UpdateExchangeRateRequest,
};
use hammer_shared::form_layout::{DeleteFormLayoutQuery, FormLayout, SaveFormLayoutRequest};
use hammer_shared::health::HealthResponse;
use hammer_shared::inventory::{
    AllocationPreview, AllocationPreviewRequest, InventoryLot, InventoryTransaction, LotQuery,
    StockBalance, StockBalanceQuery, TransactionQuery,
};
use hammer_shared::invoice::{
    InvoiceLink, InvoiceLinkQuery, UninvoicedReceipt, UninvoicedReceiptQuery, UninvoicedShipment,
    UninvoicedShipmentQuery,
};
use hammer_shared::layout::{
    CreateViewLayoutRequest, DefaultViewLayout, DeleteViewLayoutQuery, UpdateViewLayoutRequest,
    ViewLayout,
};
use hammer_shared::pagination::Page;
use hammer_shared::record::{
    CreateRecordRequest, DeleteRecordQuery, LookupItem, LookupQuery, Record, RecordListQuery,
    UpdateRecordRequest,
};
use hammer_shared::sales::{
    SalesProductLookupItem, SalesProductLookupQuery, SalesProductName, product_names_param,
};
use hammer_shared::schema::{
    ChangeFieldTypeRequest, CreateEntityRequest, CreateFieldRequest, DeactivateFieldRequest,
    DeleteEntityQuery, EntityDefinition, EntityDetail, FieldDefinition, ListEntitiesQuery,
    RecalculateResult, ReorderEntitiesRequest, ReorderFieldsRequest, UpdateEntityRequest,
    UpdateFieldRequest, validate_key,
};
use hammer_shared::setup::{CreateAdminRequest, SetupStatus};
use hammer_shared::tax_rate::TaxRate;
use hammer_shared::user::{
    CreateUserRequest, DeleteUserQuery, ResetPasswordRequest, Role, UpdateUserRequest, User,
};
use hammer_shared::{ApiError, ErrorCode, Id};
use reqwest::header::{HeaderMap, HeaderValue};
use reqwest::multipart::{Form, Part};
use reqwest::{Body, RequestBuilder, Response, StatusCode};
use serde::de::DeserializeOwned;

use crate::error::ClientError;

/// 桌面程式版本（`Cargo.toml` 的 `version`），送給伺服器做版本檢查。
pub const CLIENT_VERSION: &str = env!("CARGO_PKG_VERSION");

const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// 上傳、下載附件的逾時（檔案較大、經由 Tailscale 連線時需要較久）。
pub const TRANSFER_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// 上傳時每次讀取檔案的大小，也是回報進度的間隔。
pub const UPLOAD_CHUNK_BYTES: usize = 64 * 1024;

/// 要上傳的檔案。
pub struct UploadFile {
    /// 原始檔名（伺服器會再去除路徑與控制字元）。
    pub file_name: String,
    /// MIME；`None` 時伺服器使用 `application/octet-stream`。
    pub mime_type: Option<String>,
    /// 位元組數，必須與 `body` 的內容長度一致。
    pub size_bytes: u64,
    /// 檔案內容，見 [`progress_body`]。
    pub body: Body,
}

#[derive(Debug, Clone)]
pub struct ApiClient {
    http: reqwest::Client,
}

impl ApiClient {
    pub fn new() -> reqwest::Result<Self> {
        let mut headers = HeaderMap::new();
        headers.insert(
            CLIENT_VERSION_HEADER,
            HeaderValue::from_static(CLIENT_VERSION),
        );
        let http = reqwest::Client::builder()
            .default_headers(headers)
            .user_agent(concat!("hammer-desktop/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .build()?;
        Ok(Self { http })
    }

    /// `GET /api/v1/health`。
    ///
    /// 資料庫異常時伺服器回 `503`，內容仍是 [`HealthResponse`]，這裡當作成功回傳，
    /// 由呼叫端看 `database` 欄位判斷。
    pub async fn health(&self, server_url: &str) -> Result<HealthResponse, ClientError> {
        let resp = send(self.http.get(endpoint(server_url, "/health"))).await?;
        if resp.status() == StatusCode::SERVICE_UNAVAILABLE {
            let status = resp.status();
            let body = read_body(resp).await?;
            return serde_json::from_slice(&body).map_err(|_| error_from_body(status, &body));
        }
        read_json(resp).await
    }

    /// `GET /api/v1/setup/status`（不需要登入）。
    pub async fn setup_status(&self, server_url: &str) -> Result<SetupStatus, ClientError> {
        read_json(send(self.http.get(endpoint(server_url, "/setup/status"))).await?).await
    }

    /// `POST /api/v1/setup/admin`（不需要登入）。
    pub async fn setup_admin(
        &self,
        server_url: &str,
        req: &CreateAdminRequest,
    ) -> Result<LoginResponse, ClientError> {
        let builder = self
            .http
            .post(endpoint(server_url, "/setup/admin"))
            .json(req);
        read_json(send(builder).await?).await
    }

    /// `POST /api/v1/auth/login`（不需要登入）。
    ///
    /// 帳號或密碼錯誤時伺服器回 `401`，這裡是 [`ClientError::Api`]，不是
    /// [`ClientError::SessionExpired`]。
    pub async fn login(
        &self,
        server_url: &str,
        req: &LoginRequest,
    ) -> Result<LoginResponse, ClientError> {
        let builder = self
            .http
            .post(endpoint(server_url, "/auth/login"))
            .json(req);
        read_json(send(builder).await?).await
    }

    /// `POST /api/v1/auth/logout`。
    pub async fn logout(&self, server_url: &str, token: &str) -> Result<(), ClientError> {
        let builder = self
            .http
            .post(endpoint(server_url, "/auth/logout"))
            .bearer_auth(token);
        authed(read_empty(send(builder).await?).await)
    }

    /// `GET /api/v1/auth/me`。
    pub async fn me(&self, server_url: &str, token: &str) -> Result<CurrentUser, ClientError> {
        let builder = self
            .http
            .get(endpoint(server_url, "/auth/me"))
            .bearer_auth(token);
        authed(read_json(send(builder).await?).await)
    }

    /// `POST /api/v1/auth/change-password`。
    pub async fn change_password(
        &self,
        server_url: &str,
        token: &str,
        req: &ChangePasswordRequest,
    ) -> Result<(), ClientError> {
        let builder = self
            .http
            .post(endpoint(server_url, "/auth/change-password"))
            .bearer_auth(token)
            .json(req);
        authed(read_empty(send(builder).await?).await)
    }

    /// `GET /api/v1/roles`。
    pub async fn list_roles(
        &self,
        server_url: &str,
        token: &str,
    ) -> Result<Vec<Role>, ClientError> {
        let builder = self
            .http
            .get(endpoint(server_url, "/roles"))
            .bearer_auth(token);
        authed(read_json(send(builder).await?).await)
    }

    /// `GET /api/v1/users`。
    pub async fn list_users(
        &self,
        server_url: &str,
        token: &str,
    ) -> Result<Vec<User>, ClientError> {
        let builder = self
            .http
            .get(endpoint(server_url, "/users"))
            .bearer_auth(token);
        authed(read_json(send(builder).await?).await)
    }

    /// `POST /api/v1/users`。
    pub async fn create_user(
        &self,
        server_url: &str,
        token: &str,
        req: &CreateUserRequest,
    ) -> Result<User, ClientError> {
        let builder = self
            .http
            .post(endpoint(server_url, "/users"))
            .bearer_auth(token)
            .json(req);
        authed(read_json(send(builder).await?).await)
    }

    /// `PATCH /api/v1/users/{id}`。
    pub async fn update_user(
        &self,
        server_url: &str,
        token: &str,
        id: Id,
        req: &UpdateUserRequest,
    ) -> Result<User, ClientError> {
        let builder = self
            .http
            .patch(endpoint(server_url, &format!("/users/{id}")))
            .bearer_auth(token)
            .json(req);
        authed(read_json(send(builder).await?).await)
    }

    /// `POST /api/v1/users/{id}/reset-password`。
    pub async fn reset_user_password(
        &self,
        server_url: &str,
        token: &str,
        id: Id,
        req: &ResetPasswordRequest,
    ) -> Result<User, ClientError> {
        let builder = self
            .http
            .post(endpoint(server_url, &format!("/users/{id}/reset-password")))
            .bearer_auth(token)
            .json(req);
        authed(read_json(send(builder).await?).await)
    }

    /// `POST /api/v1/users/{id}/unlock`。
    pub async fn unlock_user(
        &self,
        server_url: &str,
        token: &str,
        id: Id,
    ) -> Result<User, ClientError> {
        let builder = self
            .http
            .post(endpoint(server_url, &format!("/users/{id}/unlock")))
            .bearer_auth(token);
        authed(read_json(send(builder).await?).await)
    }

    /// `DELETE /api/v1/users/{id}?version=N`。
    pub async fn delete_user(
        &self,
        server_url: &str,
        token: &str,
        id: Id,
        query: &DeleteUserQuery,
    ) -> Result<(), ClientError> {
        let builder = self
            .http
            .delete(endpoint(server_url, &format!("/users/{id}")))
            .bearer_auth(token)
            .query(query);
        authed(read_empty(send(builder).await?).await)
    }

    /// `GET /api/v1/audit-logs`。
    ///
    /// `None` 的條件不會放進 query string；時間的 `+` 會編碼成 `%2B`。
    pub async fn list_audit_logs(
        &self,
        server_url: &str,
        token: &str,
        query: &AuditLogQuery,
    ) -> Result<Page<AuditLogEntry>, ClientError> {
        let builder = self
            .http
            .get(endpoint(server_url, "/audit-logs"))
            .bearer_auth(token)
            .query(query);
        authed(read_json(send(builder).await?).await)
    }

    /// `GET /api/v1/inventory/balances`（商品庫存）。
    pub async fn list_stock_balances(
        &self,
        server_url: &str,
        token: &str,
        query: &StockBalanceQuery,
    ) -> Result<Page<StockBalance>, ClientError> {
        let builder = self
            .http
            .get(endpoint(server_url, "/inventory/balances"))
            .bearer_auth(token)
            .query(query);
        authed(read_json(send(builder).await?).await)
    }

    /// `GET /api/v1/inventory/products/{product_id}/lots`（商品的批號，先進先出順序）。
    pub async fn list_product_lots(
        &self,
        server_url: &str,
        token: &str,
        product_id: Id,
        query: &LotQuery,
    ) -> Result<Page<InventoryLot>, ClientError> {
        let builder = self
            .http
            .get(endpoint(
                server_url,
                &format!("/inventory/products/{product_id}/lots"),
            ))
            .bearer_auth(token)
            .query(query);
        authed(read_json(send(builder).await?).await)
    }

    /// `GET /api/v1/inventory/transactions`（異動流水帳，新到舊）。
    pub async fn list_inventory_transactions(
        &self,
        server_url: &str,
        token: &str,
        query: &TransactionQuery,
    ) -> Result<Page<InventoryTransaction>, ClientError> {
        let builder = self
            .http
            .get(endpoint(server_url, "/inventory/transactions"))
            .bearer_auth(token)
            .query(query);
        authed(read_json(send(builder).await?).await)
    }

    /// `GET /api/v1/invoicing/uninvoiced-shipments`（尚未開立發票的出貨單）。
    pub async fn list_uninvoiced_shipments(
        &self,
        server_url: &str,
        token: &str,
        query: &UninvoicedShipmentQuery,
    ) -> Result<Page<UninvoicedShipment>, ClientError> {
        let builder = self
            .http
            .get(endpoint(server_url, "/invoicing/uninvoiced-shipments"))
            .bearer_auth(token)
            .query(query);
        authed(read_json(send(builder).await?).await)
    }

    /// `GET /api/v1/invoicing/uninvoiced-receipts`（尚未取得憑證的進貨單）。
    pub async fn list_uninvoiced_receipts(
        &self,
        server_url: &str,
        token: &str,
        query: &UninvoicedReceiptQuery,
    ) -> Result<Page<UninvoicedReceipt>, ClientError> {
        let builder = self
            .http
            .get(endpoint(server_url, "/invoicing/uninvoiced-receipts"))
            .bearer_auth(token)
            .query(query);
        authed(read_json(send(builder).await?).await)
    }

    /// `GET /api/v1/invoicing/links`（單據的發票、憑證、折讓）。
    pub async fn list_invoice_links(
        &self,
        server_url: &str,
        token: &str,
        query: &InvoiceLinkQuery,
    ) -> Result<Vec<InvoiceLink>, ClientError> {
        let builder = self
            .http
            .get(endpoint(server_url, "/invoicing/links"))
            .bearer_auth(token)
            .query(query);
        authed(read_json(send(builder).await?).await)
    }

    /// `POST /api/v1/inventory/allocation-preview`（出貨的建議批號，只試算）。
    pub async fn allocation_preview(
        &self,
        server_url: &str,
        token: &str,
        req: &AllocationPreviewRequest,
    ) -> Result<AllocationPreview, ClientError> {
        let builder = self
            .http
            .post(endpoint(server_url, "/inventory/allocation-preview"))
            .bearer_auth(token)
            .json(req);
        authed(read_json(send(builder).await?).await)
    }

    /// `GET /api/v1/entities?include_inactive=…`。
    pub async fn list_entities(
        &self,
        server_url: &str,
        token: &str,
        query: &ListEntitiesQuery,
    ) -> Result<Vec<EntityDefinition>, ClientError> {
        let builder = self
            .http
            .get(endpoint(server_url, "/entities"))
            .bearer_auth(token)
            .query(query);
        authed(read_json(send(builder).await?).await)
    }

    /// `GET /api/v1/currencies`（本位幣優先、再依代碼排序）。
    pub async fn list_currencies(
        &self,
        server_url: &str,
        token: &str,
        query: &CurrencyListQuery,
    ) -> Result<Vec<Currency>, ClientError> {
        let builder = self
            .http
            .get(endpoint(server_url, "/currencies"))
            .bearer_auth(token)
            .query(query);
        authed(read_json(send(builder).await?).await)
    }

    /// `POST /api/v1/currencies`。
    pub async fn create_currency(
        &self,
        server_url: &str,
        token: &str,
        req: &CreateCurrencyRequest,
    ) -> Result<Currency, ClientError> {
        let builder = self
            .http
            .post(endpoint(server_url, "/currencies"))
            .bearer_auth(token)
            .json(req);
        authed(read_json(send(builder).await?).await)
    }

    /// `PATCH /api/v1/currencies/{code}`。
    pub async fn update_currency(
        &self,
        server_url: &str,
        token: &str,
        code: &str,
        req: &UpdateCurrencyRequest,
    ) -> Result<Currency, ClientError> {
        let builder = self
            .http
            .patch(currency_endpoint(server_url, code)?)
            .bearer_auth(token)
            .json(req);
        authed(read_json(send(builder).await?).await)
    }

    /// `GET /api/v1/exchange-rates`（分頁，依日期新到舊）。`None` 的條件不會放進 query string。
    pub async fn list_exchange_rates(
        &self,
        server_url: &str,
        token: &str,
        query: &ExchangeRateQuery,
    ) -> Result<Page<ExchangeRate>, ClientError> {
        let builder = self
            .http
            .get(endpoint(server_url, "/exchange-rates"))
            .bearer_auth(token)
            .query(query);
        authed(read_json(send(builder).await?).await)
    }

    /// `GET /api/v1/exchange-rates/effective`：幣別在單據日期的適用匯率（找不到時 `rate` 為 `null`）。
    pub async fn effective_exchange_rate(
        &self,
        server_url: &str,
        token: &str,
        query: &EffectiveRateQuery,
    ) -> Result<EffectiveRate, ClientError> {
        let builder = self
            .http
            .get(endpoint(server_url, "/exchange-rates/effective"))
            .bearer_auth(token)
            .query(query);
        authed(read_json(send(builder).await?).await)
    }

    /// `GET /api/v1/tax-rates`（全部稅別）。
    pub async fn list_tax_rates(
        &self,
        server_url: &str,
        token: &str,
    ) -> Result<Vec<TaxRate>, ClientError> {
        let builder = self
            .http
            .get(endpoint(server_url, "/tax-rates"))
            .bearer_auth(token);
        authed(read_json(send(builder).await?).await)
    }

    /// `POST /api/v1/exchange-rates`。
    pub async fn create_exchange_rate(
        &self,
        server_url: &str,
        token: &str,
        req: &CreateExchangeRateRequest,
    ) -> Result<ExchangeRate, ClientError> {
        let builder = self
            .http
            .post(endpoint(server_url, "/exchange-rates"))
            .bearer_auth(token)
            .json(req);
        authed(read_json(send(builder).await?).await)
    }

    /// `PATCH /api/v1/exchange-rates/{id}`。
    pub async fn update_exchange_rate(
        &self,
        server_url: &str,
        token: &str,
        id: Id,
        req: &UpdateExchangeRateRequest,
    ) -> Result<ExchangeRate, ClientError> {
        let builder = self
            .http
            .patch(endpoint(server_url, &format!("/exchange-rates/{id}")))
            .bearer_auth(token)
            .json(req);
        authed(read_json(send(builder).await?).await)
    }

    /// `DELETE /api/v1/exchange-rates/{id}?version=N`。
    pub async fn delete_exchange_rate(
        &self,
        server_url: &str,
        token: &str,
        id: Id,
        query: &DeleteExchangeRateQuery,
    ) -> Result<(), ClientError> {
        let builder = self
            .http
            .delete(endpoint(server_url, &format!("/exchange-rates/{id}")))
            .bearer_auth(token)
            .query(query);
        authed(read_empty(send(builder).await?).await)
    }

    /// `GET /api/v1/entities/{key}`。
    pub async fn get_entity(
        &self,
        server_url: &str,
        token: &str,
        key: &str,
    ) -> Result<EntityDetail, ClientError> {
        let builder = self
            .http
            .get(entity_endpoint(server_url, key, None, "")?)
            .bearer_auth(token);
        authed(read_json(send(builder).await?).await)
    }

    /// `POST /api/v1/entities`。
    pub async fn create_entity(
        &self,
        server_url: &str,
        token: &str,
        req: &CreateEntityRequest,
    ) -> Result<EntityDetail, ClientError> {
        let builder = self
            .http
            .post(endpoint(server_url, "/entities"))
            .bearer_auth(token)
            .json(req);
        authed(read_json(send(builder).await?).await)
    }

    /// `PATCH /api/v1/entities/{key}`。
    pub async fn update_entity(
        &self,
        server_url: &str,
        token: &str,
        key: &str,
        req: &UpdateEntityRequest,
    ) -> Result<EntityDetail, ClientError> {
        let builder = self
            .http
            .patch(entity_endpoint(server_url, key, None, "")?)
            .bearer_auth(token)
            .json(req);
        authed(read_json(send(builder).await?).await)
    }

    /// `PUT /api/v1/entities/order`。
    pub async fn reorder_entities(
        &self,
        server_url: &str,
        token: &str,
        req: &ReorderEntitiesRequest,
    ) -> Result<Vec<EntityDefinition>, ClientError> {
        let builder = self
            .http
            .put(endpoint(server_url, "/entities/order"))
            .bearer_auth(token)
            .json(req);
        authed(read_json(send(builder).await?).await)
    }

    /// `DELETE /api/v1/entities/{key}?version=N`。
    pub async fn delete_entity(
        &self,
        server_url: &str,
        token: &str,
        key: &str,
        query: &DeleteEntityQuery,
    ) -> Result<(), ClientError> {
        let builder = self
            .http
            .delete(entity_endpoint(server_url, key, None, "")?)
            .bearer_auth(token)
            .query(query);
        authed(read_empty(send(builder).await?).await)
    }

    /// `POST /api/v1/entities/{key}/recalculate`。
    pub async fn recalculate_entity(
        &self,
        server_url: &str,
        token: &str,
        key: &str,
    ) -> Result<RecalculateResult, ClientError> {
        let builder = self
            .http
            .post(entity_endpoint(server_url, key, None, "/recalculate")?)
            .bearer_auth(token);
        authed(read_json(send(builder).await?).await)
    }

    /// `POST /api/v1/entities/{key}/fields`。
    pub async fn create_field(
        &self,
        server_url: &str,
        token: &str,
        key: &str,
        req: &CreateFieldRequest,
    ) -> Result<FieldDefinition, ClientError> {
        let builder = self
            .http
            .post(entity_endpoint(server_url, key, None, "/fields")?)
            .bearer_auth(token)
            .json(req);
        authed(read_json(send(builder).await?).await)
    }

    /// `PATCH /api/v1/entities/{key}/fields/{field_key}`。
    pub async fn update_field(
        &self,
        server_url: &str,
        token: &str,
        key: &str,
        field_key: &str,
        req: &UpdateFieldRequest,
    ) -> Result<FieldDefinition, ClientError> {
        let builder = self
            .http
            .patch(entity_endpoint(server_url, key, Some(field_key), "")?)
            .bearer_auth(token)
            .json(req);
        authed(read_json(send(builder).await?).await)
    }

    /// `POST /api/v1/entities/{key}/fields/{field_key}/deactivate`。
    pub async fn deactivate_field(
        &self,
        server_url: &str,
        token: &str,
        key: &str,
        field_key: &str,
        req: &DeactivateFieldRequest,
    ) -> Result<FieldDefinition, ClientError> {
        let builder = self
            .http
            .post(entity_endpoint(
                server_url,
                key,
                Some(field_key),
                "/deactivate",
            )?)
            .bearer_auth(token)
            .json(req);
        authed(read_json(send(builder).await?).await)
    }

    /// `PUT /api/v1/entities/{key}/fields/order`。
    pub async fn reorder_fields(
        &self,
        server_url: &str,
        token: &str,
        key: &str,
        req: &ReorderFieldsRequest,
    ) -> Result<Vec<FieldDefinition>, ClientError> {
        let builder = self
            .http
            .put(entity_endpoint(server_url, key, None, "/fields/order")?)
            .bearer_auth(token)
            .json(req);
        authed(read_json(send(builder).await?).await)
    }

    /// `POST /api/v1/entities/{key}/fields/{field_key}/change-type`。
    pub async fn change_field_type(
        &self,
        server_url: &str,
        token: &str,
        key: &str,
        field_key: &str,
        req: &ChangeFieldTypeRequest,
    ) -> Result<FieldDefinition, ClientError> {
        let builder = self
            .http
            .post(entity_endpoint(
                server_url,
                key,
                Some(field_key),
                "/change-type",
            )?)
            .bearer_auth(token)
            .json(req);
        authed(read_json(send(builder).await?).await)
    }
    /// `GET /api/v1/entities/{key}/records`。
    ///
    /// 篩選參數名稱是動態的（`filter.<field_key>.<op>`），以 [`RecordListQuery::to_pairs`]
    /// 產生 query string。
    pub async fn list_records(
        &self,
        server_url: &str,
        token: &str,
        key: &str,
        query: &RecordListQuery,
    ) -> Result<Page<Record>, ClientError> {
        let builder = self
            .http
            .get(entity_endpoint(server_url, key, None, "/records")?)
            .bearer_auth(token)
            .query(&query.to_pairs());
        authed(read_json(send(builder).await?).await)
    }

    /// `GET /api/v1/entities/{key}/records/{id}`。
    pub async fn get_record(
        &self,
        server_url: &str,
        token: &str,
        key: &str,
        id: Id,
    ) -> Result<Record, ClientError> {
        let builder = self
            .http
            .get(entity_endpoint(
                server_url,
                key,
                None,
                &format!("/records/{id}"),
            )?)
            .bearer_auth(token);
        authed(read_json(send(builder).await?).await)
    }

    /// `POST /api/v1/entities/{key}/records`。
    pub async fn create_record(
        &self,
        server_url: &str,
        token: &str,
        key: &str,
        req: &CreateRecordRequest,
    ) -> Result<Record, ClientError> {
        let builder = self
            .http
            .post(entity_endpoint(server_url, key, None, "/records")?)
            .bearer_auth(token)
            .json(req);
        authed(read_json(send(builder).await?).await)
    }

    /// `PATCH /api/v1/entities/{key}/records/{id}`。
    pub async fn update_record(
        &self,
        server_url: &str,
        token: &str,
        key: &str,
        id: Id,
        req: &UpdateRecordRequest,
    ) -> Result<Record, ClientError> {
        let builder = self
            .http
            .patch(entity_endpoint(
                server_url,
                key,
                None,
                &format!("/records/{id}"),
            )?)
            .bearer_auth(token)
            .json(req);
        authed(read_json(send(builder).await?).await)
    }

    /// `DELETE /api/v1/entities/{key}/records/{id}?version=N`。
    pub async fn delete_record(
        &self,
        server_url: &str,
        token: &str,
        key: &str,
        id: Id,
        query: &DeleteRecordQuery,
    ) -> Result<(), ClientError> {
        let builder = self
            .http
            .delete(entity_endpoint(
                server_url,
                key,
                None,
                &format!("/records/{id}"),
            )?)
            .bearer_auth(token)
            .query(query);
        authed(read_empty(send(builder).await?).await)
    }

    /// `POST /api/v1/entities/{key}/records/{id}/post`、`…/void`（系統單據的過帳、作廢）。
    pub async fn document_action(
        &self,
        server_url: &str,
        token: &str,
        key: &str,
        id: Id,
        action: DocumentAction,
        req: &DocumentActionRequest,
    ) -> Result<Record, ClientError> {
        let builder = self
            .http
            .post(entity_endpoint(
                server_url,
                key,
                None,
                &format!("/records/{id}/{}", action.as_str()),
            )?)
            .bearer_auth(token)
            .json(req);
        authed(read_json(send(builder).await?).await)
    }

    /// `GET /api/v1/entities/{key}/layouts`。
    pub async fn list_layouts(
        &self,
        server_url: &str,
        token: &str,
        key: &str,
    ) -> Result<Vec<ViewLayout>, ClientError> {
        let builder = self
            .http
            .get(entity_endpoint(server_url, key, None, "/layouts")?)
            .bearer_auth(token);
        authed(read_json(send(builder).await?).await)
    }

    /// `GET /api/v1/entities/{key}/layouts/default`。
    pub async fn get_default_layout(
        &self,
        server_url: &str,
        token: &str,
        key: &str,
    ) -> Result<DefaultViewLayout, ClientError> {
        let builder = self
            .http
            .get(entity_endpoint(server_url, key, None, "/layouts/default")?)
            .bearer_auth(token);
        authed(read_json(send(builder).await?).await)
    }

    /// `POST /api/v1/entities/{key}/layouts`。
    pub async fn create_layout(
        &self,
        server_url: &str,
        token: &str,
        key: &str,
        req: &CreateViewLayoutRequest,
    ) -> Result<ViewLayout, ClientError> {
        let builder = self
            .http
            .post(entity_endpoint(server_url, key, None, "/layouts")?)
            .bearer_auth(token)
            .json(req);
        authed(read_json(send(builder).await?).await)
    }

    /// `PATCH /api/v1/entities/{key}/layouts/{id}`。
    pub async fn update_layout(
        &self,
        server_url: &str,
        token: &str,
        key: &str,
        id: Id,
        req: &UpdateViewLayoutRequest,
    ) -> Result<ViewLayout, ClientError> {
        let builder = self
            .http
            .patch(entity_endpoint(
                server_url,
                key,
                None,
                &format!("/layouts/{id}"),
            )?)
            .bearer_auth(token)
            .json(req);
        authed(read_json(send(builder).await?).await)
    }

    /// `DELETE /api/v1/entities/{key}/layouts/{id}?version=N`。
    pub async fn delete_layout(
        &self,
        server_url: &str,
        token: &str,
        key: &str,
        id: Id,
        query: &DeleteViewLayoutQuery,
    ) -> Result<(), ClientError> {
        let builder = self
            .http
            .delete(entity_endpoint(
                server_url,
                key,
                None,
                &format!("/layouts/{id}"),
            )?)
            .bearer_auth(token)
            .query(query);
        authed(read_empty(send(builder).await?).await)
    }

    /// `GET /api/v1/entities/{key}/form-layout`。
    pub async fn get_form_layout(
        &self,
        server_url: &str,
        token: &str,
        key: &str,
    ) -> Result<FormLayout, ClientError> {
        let builder = self
            .http
            .get(entity_endpoint(server_url, key, None, "/form-layout")?)
            .bearer_auth(token);
        authed(read_json(send(builder).await?).await)
    }

    /// `PUT /api/v1/entities/{key}/form-layout`。
    pub async fn save_form_layout(
        &self,
        server_url: &str,
        token: &str,
        key: &str,
        req: &SaveFormLayoutRequest,
    ) -> Result<FormLayout, ClientError> {
        let builder = self
            .http
            .put(entity_endpoint(server_url, key, None, "/form-layout")?)
            .bearer_auth(token)
            .json(req);
        authed(read_json(send(builder).await?).await)
    }

    /// `DELETE /api/v1/entities/{key}/form-layout?version=N`（還原成預設）。
    pub async fn reset_form_layout(
        &self,
        server_url: &str,
        token: &str,
        key: &str,
        query: &DeleteFormLayoutQuery,
    ) -> Result<FormLayout, ClientError> {
        let builder = self
            .http
            .delete(entity_endpoint(server_url, key, None, "/form-layout")?)
            .bearer_auth(token)
            .query(query);
        authed(read_json(send(builder).await?).await)
    }

    /// `GET /api/v1/entities/{key}/records/lookup?q=&limit=`（關聯欄位的選擇器搜尋）。
    pub async fn lookup_records(
        &self,
        server_url: &str,
        token: &str,
        key: &str,
        query: &LookupQuery,
    ) -> Result<Vec<LookupItem>, ClientError> {
        let builder = self
            .http
            .get(entity_endpoint(server_url, key, None, "/records/lookup")?)
            .bearer_auth(token)
            .query(query);
        authed(read_json(send(builder).await?).await)
    }

    /// `GET /api/v1/sales/product-lookup`（銷貨用商品選擇，可用客戶品名搜尋）。
    pub async fn sales_product_lookup(
        &self,
        server_url: &str,
        token: &str,
        query: &SalesProductLookupQuery,
    ) -> Result<Vec<SalesProductLookupItem>, ClientError> {
        let builder = self
            .http
            .get(endpoint(server_url, "/sales/product-lookup"))
            .bearer_auth(token)
            .query(query);
        authed(read_json(send(builder).await?).await)
    }

    /// `GET /api/v1/sales/product-names?customer_id=&product_id=…`（批次取得明細品名，M12）。
    pub async fn sales_product_names(
        &self,
        server_url: &str,
        token: &str,
        customer_id: Id,
        product_ids: &[Id],
    ) -> Result<Vec<SalesProductName>, ClientError> {
        let mut pairs = vec![(product_names_param::CUSTOMER_ID, customer_id.to_string())];
        pairs.extend(
            product_ids
                .iter()
                .map(|id| (product_names_param::PRODUCT_ID, id.to_string())),
        );
        let builder = self
            .http
            .get(endpoint(server_url, "/sales/product-names"))
            .bearer_auth(token)
            .query(&pairs);
        authed(read_json(send(builder).await?).await)
    }

    /// `POST /api/v1/attachments`（`multipart/form-data`，檔案在 [`UPLOAD_FIELD`] 欄位）。
    ///
    /// 超過伺服器的大小上限時回傳 `413 PAYLOAD_TOO_LARGE`。
    pub async fn upload_attachment(
        &self,
        server_url: &str,
        token: &str,
        file: UploadFile,
    ) -> Result<Attachment, ClientError> {
        let mut part =
            Part::stream_with_length(file.body, file.size_bytes).file_name(file.file_name);
        if let Some(mime) = file.mime_type {
            part = part
                .mime_str(&mime)
                .map_err(|_| ClientError::invalid_input(format!("檔案類型「{mime}」不正確")))?;
        }
        let builder = self
            .http
            .post(endpoint(server_url, "/attachments"))
            .bearer_auth(token)
            .timeout(TRANSFER_TIMEOUT)
            .multipart(Form::new().part(UPLOAD_FIELD, part));
        authed(read_json(send(builder).await?).await)
    }

    /// `GET /api/v1/attachments/{id}`，回傳檔案內容。附件不存在或已刪除時回傳 `404 NOT_FOUND`。
    pub async fn download_attachment(
        &self,
        server_url: &str,
        token: &str,
        id: Id,
    ) -> Result<Vec<u8>, ClientError> {
        let builder = self
            .http
            .get(endpoint(server_url, &format!("/attachments/{id}")))
            .bearer_auth(token)
            .timeout(TRANSFER_TIMEOUT);
        let resp = send(builder).await?;
        let status = resp.status();
        let body = read_body(resp).await?;
        if status.is_success() {
            Ok(body)
        } else {
            authed(Err(error_from_body(status, &body)))
        }
    }
}

/// 以 [`UPLOAD_CHUNK_BYTES`] 為單位讀取 `reader` 作為上傳的內容；每讀取一段呼叫一次
/// `on_progress(已讀取的位元組數)`。讀取失敗時上傳中止。
pub fn progress_body<R>(reader: R, on_progress: impl FnMut(u64) + Send + 'static) -> Body
where
    R: Read + Send + 'static,
{
    let stream = futures_util::stream::unfold(
        (reader, 0u64, on_progress, false),
        |(mut reader, sent, mut on_progress, failed)| async move {
            if failed {
                return None;
            }
            let mut buf = vec![0; UPLOAD_CHUNK_BYTES];
            match reader.read(&mut buf) {
                Ok(0) => None,
                Ok(n) => {
                    buf.truncate(n);
                    let sent = sent + n as u64;
                    on_progress(sent);
                    Some((Ok(buf), (reader, sent, on_progress, false)))
                }
                Err(err) => Some((Err(err), (reader, sent, on_progress, true))),
            }
        },
    );
    Body::wrap_stream(stream)
}

async fn send(builder: RequestBuilder) -> Result<Response, ClientError> {
    builder.send().await.map_err(network_error)
}

/// 需要登入的請求：伺服器回 `401 UNAUTHORIZED` 表示 session 已失效。
fn authed<T>(result: Result<T, ClientError>) -> Result<T, ClientError> {
    match result {
        Err(ClientError::Api(ApiError {
            code: ErrorCode::Unauthorized,
            ..
        })) => Err(ClientError::session_expired()),
        other => other,
    }
}

/// `http://host:8080` + `/health` → `http://host:8080/api/v1/health`。
fn endpoint(server_url: &str, path: &str) -> String {
    format!("{}{API_PREFIX}{path}", server_url.trim_end_matches('/'))
}

/// `/api/v1/entities/{key}`，有 `field_key` 時加上 `/fields/{field_key}`，最後加上 `suffix`。
///
/// 代碼（`key`、`field_key`）必須符合命名規則（只有 `a-z`、`0-9`、`_`），因此不需要編碼，
/// 也不會改變路徑結構；不符合時回傳 [`ClientError::InvalidInput`]，不送出請求。
fn entity_endpoint(
    server_url: &str,
    key: &str,
    field_key: Option<&str>,
    suffix: &str,
) -> Result<String, ClientError> {
    let mut path = String::from("/entities/");
    for (i, code) in std::iter::once(key).chain(field_key).enumerate() {
        validate_key(code)
            .map_err(|e| ClientError::invalid_input(format!("代碼「{code}」不正確：{e}")))?;
        if i > 0 {
            path.push_str("/fields/");
        }
        path.push_str(code);
    }
    path.push_str(suffix);
    Ok(endpoint(server_url, &path))
}

/// `/api/v1/currencies/{code}`。
///
/// 幣別代碼只有英文字母（伺服器規定 3 個大寫字母），因此不需要編碼；
/// 含其他字元時回傳 [`ClientError::InvalidInput`]，不送出請求。
fn currency_endpoint(server_url: &str, code: &str) -> Result<String, ClientError> {
    if code.is_empty() || !code.chars().all(|c| c.is_ascii_alphabetic()) {
        return Err(ClientError::invalid_input(format!(
            "幣別代碼「{code}」不正確"
        )));
    }
    Ok(endpoint(server_url, &format!("/currencies/{code}")))
}

/// 成功時解析成 `T`；失敗時轉成 [`ClientError`]。
async fn read_json<T: DeserializeOwned>(resp: Response) -> Result<T, ClientError> {
    let status = resp.status();
    let body = read_body(resp).await?;
    if !status.is_success() {
        return Err(error_from_body(status, &body));
    }
    serde_json::from_slice(&body).map_err(|_| ClientError::InvalidResponse {
        message: format!(
            "伺服器回應格式不正確（HTTP {}），請確認伺服器網址",
            status.as_u16()
        ),
    })
}

/// 沒有回應內容的端點（`204 No Content`）。
async fn read_empty(resp: Response) -> Result<(), ClientError> {
    let status = resp.status();
    if status.is_success() {
        return Ok(());
    }
    let body = read_body(resp).await?;
    Err(error_from_body(status, &body))
}

async fn read_body(resp: Response) -> Result<Vec<u8>, ClientError> {
    resp.bytes()
        .await
        .map(|b| b.to_vec())
        .map_err(network_error)
}

/// 錯誤回應：統一錯誤格式 → [`ClientError::Api`]；其他 → [`ClientError::InvalidResponse`]。
fn error_from_body(status: StatusCode, body: &[u8]) -> ClientError {
    match serde_json::from_slice::<ApiError>(body) {
        Ok(err) => ClientError::Api(err),
        Err(_) => ClientError::InvalidResponse {
            message: format!(
                "伺服器回應錯誤（HTTP {}），請確認伺服器網址",
                status.as_u16()
            ),
        },
    }
}

fn network_error(err: reqwest::Error) -> ClientError {
    let message = if err.is_timeout() {
        "連線逾時，請確認伺服器是否正常運作"
    } else if err.is_connect() {
        "無法連線到伺服器，請確認伺服器已啟動、網址正確"
    } else {
        "與伺服器通訊失敗"
    };
    ClientError::Network {
        message: message.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::http::{HeaderMap as AxumHeaders, StatusCode as AxumStatus};
    use axum::routing::{get, post};
    use hammer_shared::ErrorCode;
    use hammer_shared::health::HealthStatus;
    use serde_json::json;

    /// 在隨機埠啟動測試用伺服器，回傳網址。
    async fn serve(api: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let app = Router::new().nest(API_PREFIX, api);
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}")
    }

    fn health_body(database: &str) -> serde_json::Value {
        json!({ "status": database, "version": "9.9.9", "database": database })
    }

    #[test]
    fn endpoint_joins_prefix() {
        assert_eq!(
            endpoint("http://localhost:8080", "/health"),
            "http://localhost:8080/api/v1/health"
        );
        assert_eq!(
            endpoint("http://h/erp/", "/health"),
            "http://h/erp/api/v1/health"
        );
    }

    #[test]
    fn entity_endpoint_checks_codes() {
        assert_eq!(
            entity_endpoint("http://h", "repair", None, "").unwrap(),
            "http://h/api/v1/entities/repair"
        );
        assert_eq!(
            entity_endpoint("http://h", "repair", Some("order"), "/change-type").unwrap(),
            "http://h/api/v1/entities/repair/fields/order/change-type"
        );
        assert_eq!(
            entity_endpoint("http://h", "repair", None, "/fields/order").unwrap(),
            "http://h/api/v1/entities/repair/fields/order"
        );
        for (key, field_key) in [("..", None), ("a/b", None), ("repair", Some("../x"))] {
            assert!(matches!(
                entity_endpoint("http://h", key, field_key, ""),
                Err(ClientError::InvalidInput { .. })
            ));
        }
    }

    #[tokio::test]
    async fn health_ok_and_sends_client_version() {
        let url = serve(Router::new().route(
            "/health",
            get(|headers: AxumHeaders| async move {
                let version = headers
                    .get(CLIENT_VERSION_HEADER)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or_default()
                    .to_owned();
                // 把收到的版本放進 version 欄位，方便驗證
                axum::Json(json!({ "status": "ok", "version": version, "database": "ok" }))
            }),
        ))
        .await;

        let health = ApiClient::new().unwrap().health(&url).await.unwrap();
        assert_eq!(health.status, HealthStatus::Ok);
        assert_eq!(health.version, CLIENT_VERSION);
    }

    #[tokio::test]
    async fn sales_product_names_repeats_product_id() {
        use axum::extract::RawQuery;
        let url = serve(Router::new().route(
            "/sales/product-names",
            get(|RawQuery(query): RawQuery| async move {
                // 把收到的 query string 放進品名，方便驗證
                axum::Json(
                    json!([{ "product_id": "01a211f5-20e9-7b08-9d17-2a0dbce98c13",
                                    "customer_product_name": null,
                                    "product_name": query.unwrap_or_default() }]),
                )
            }),
        ))
        .await;
        let c = Id::try_parse("01a211f5-20e9-7b08-9d17-2a0dbce98c12").unwrap();
        let p1 = Id::try_parse("01a211f5-20e9-7b08-9d17-2a0dbce98c13").unwrap();
        let p2 = Id::try_parse("01a211f5-20e9-7b08-9d17-2a0dbce98c14").unwrap();
        let names = ApiClient::new()
            .unwrap()
            .sales_product_names(&url, "t", c, &[p1, p2])
            .await
            .unwrap();
        assert_eq!(
            names[0].product_name,
            format!("customer_id={c}&product_id={p1}&product_id={p2}")
        );
    }

    #[tokio::test]
    async fn health_503_is_returned_as_database_error() {
        let url = serve(Router::new().route(
            "/health",
            get(|| async {
                (
                    AxumStatus::SERVICE_UNAVAILABLE,
                    axum::Json(health_body("error")),
                )
            }),
        ))
        .await;

        let health = ApiClient::new().unwrap().health(&url).await.unwrap();
        assert_eq!(health.database, HealthStatus::Error);
        assert_eq!(health.version, "9.9.9");
    }

    #[tokio::test]
    async fn server_error_becomes_api_error() {
        let url = serve(Router::new().route(
            "/health",
            get(|| async {
                (
                    AxumStatus::UPGRADE_REQUIRED,
                    axum::Json(json!({
                        "code": "CLIENT_OUTDATED",
                        "message": "請更新程式",
                        "details": { "min_version": "1.0.0" }
                    })),
                )
            }),
        ))
        .await;

        let err = ApiClient::new().unwrap().health(&url).await.unwrap_err();
        let ClientError::Api(api) = err else {
            panic!("應該是 ApiError：{err:?}");
        };
        assert_eq!(api.code, ErrorCode::ClientOutdated);
        assert_eq!(api.details, Some(json!({ "min_version": "1.0.0" })));
    }

    #[tokio::test]
    async fn non_json_response_is_invalid_response() {
        let url = serve(Router::new().route("/health", get(|| async { "hello" }))).await;
        let err = ApiClient::new().unwrap().health(&url).await.unwrap_err();
        assert!(
            matches!(err, ClientError::InvalidResponse { .. }),
            "{err:?}"
        );

        // 404 且不是統一錯誤格式（例如網址指到別的服務）
        let url = serve(Router::new()).await;
        let err = ApiClient::new().unwrap().health(&url).await.unwrap_err();
        assert!(
            matches!(err, ClientError::InvalidResponse { .. }),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn unreachable_server_is_network_error() {
        // 綁定後立刻關閉，取得一個沒有人在聽的埠
        let addr = {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            listener.local_addr().unwrap()
        };
        let err = ApiClient::new()
            .unwrap()
            .health(&format!("http://{addr}"))
            .await
            .unwrap_err();
        assert!(matches!(err, ClientError::Network { .. }), "{err:?}");
    }

    fn user_json() -> serde_json::Value {
        json!({
            "id": "0196f5a0-0000-7000-8000-000000000001",
            "username": "admin",
            "display_name": "管理者",
            "role": "admin",
            "permissions": ["user.manage"],
            "must_change_password": false,
        })
    }

    fn bearer(headers: &AxumHeaders) -> String {
        headers
            .get("authorization")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned()
    }

    fn unauthorized(message: &str) -> (AxumStatus, axum::Json<serde_json::Value>) {
        (
            AxumStatus::UNAUTHORIZED,
            axum::Json(json!({ "code": "UNAUTHORIZED", "message": message })),
        )
    }

    #[tokio::test]
    async fn login_posts_credentials_and_returns_token() {
        let url = serve(Router::new().route(
            "/auth/login",
            post(
                |axum::Json(body): axum::Json<serde_json::Value>| async move {
                    assert_eq!(body, json!({ "username": "admin", "password": "pw" }));
                    axum::Json(json!({ "token": "tok", "user": user_json() }))
                },
            ),
        ))
        .await;

        let resp = ApiClient::new()
            .unwrap()
            .login(
                &url,
                &LoginRequest {
                    username: "admin".into(),
                    password: "pw".into(),
                },
            )
            .await
            .unwrap();
        assert_eq!(resp.token, "tok");
        assert_eq!(resp.user.username, "admin");
    }

    #[tokio::test]
    async fn login_401_stays_api_error() {
        // 帳號或密碼錯誤不是 session 逾時
        let url = serve(Router::new().route(
            "/auth/login",
            post(|| async { unauthorized("帳號或密碼錯誤") }),
        ))
        .await;
        let err = ApiClient::new()
            .unwrap()
            .login(
                &url,
                &LoginRequest {
                    username: "a".into(),
                    password: "b".into(),
                },
            )
            .await
            .unwrap_err();
        let ClientError::Api(api) = err else {
            panic!("應該是 ApiError：{err:?}");
        };
        assert_eq!(api.code, ErrorCode::Unauthorized);
        assert_eq!(api.message, "帳號或密碼錯誤");
    }

    #[tokio::test]
    async fn me_sends_bearer_token() {
        let url = serve(Router::new().route(
            "/auth/me",
            get(|headers: AxumHeaders| async move {
                assert_eq!(bearer(&headers), "Bearer tok");
                axum::Json(user_json())
            }),
        ))
        .await;
        let user = ApiClient::new().unwrap().me(&url, "tok").await.unwrap();
        assert_eq!(user.display_name, "管理者");
    }

    #[tokio::test]
    async fn authed_401_becomes_session_expired() {
        let url = serve(
            Router::new()
                .route("/auth/me", get(|| async { unauthorized("請先登入") }))
                .route(
                    "/auth/change-password",
                    post(|| async { unauthorized("請先登入") }),
                ),
        )
        .await;
        let client = ApiClient::new().unwrap();
        assert_eq!(
            client.me(&url, "old").await.unwrap_err(),
            ClientError::session_expired()
        );
        let req = ChangePasswordRequest {
            current_password: "a".into(),
            new_password: "b".into(),
        };
        assert_eq!(
            client.change_password(&url, "old", &req).await.unwrap_err(),
            ClientError::session_expired()
        );
    }

    #[tokio::test]
    async fn no_content_endpoints() {
        let url = serve(
            Router::new()
                .route(
                    "/auth/logout",
                    post(|headers: AxumHeaders| async move {
                        assert_eq!(bearer(&headers), "Bearer tok");
                        AxumStatus::NO_CONTENT
                    }),
                )
                .route(
                    "/auth/change-password",
                    post(|| async {
                        (
                            AxumStatus::UNPROCESSABLE_ENTITY,
                            axum::Json(
                                json!({ "code": "VALIDATION", "message": "目前的密碼不正確" }),
                            ),
                        )
                    }),
                ),
        )
        .await;
        let client = ApiClient::new().unwrap();
        client.logout(&url, "tok").await.unwrap();

        let req = ChangePasswordRequest {
            current_password: "a".into(),
            new_password: "b".into(),
        };
        let err = client.change_password(&url, "tok", &req).await.unwrap_err();
        let ClientError::Api(api) = err else {
            panic!("應該是 ApiError：{err:?}");
        };
        assert_eq!(api.code, ErrorCode::Validation);
    }

    #[tokio::test]
    async fn setup_endpoints() {
        let url = serve(
            Router::new()
                .route(
                    "/setup/status",
                    get(|| async { axum::Json(json!({ "needs_setup": true })) }),
                )
                .route(
                    "/setup/admin",
                    post(
                        |axum::Json(body): axum::Json<serde_json::Value>| async move {
                            assert_eq!(body["display_name"], "管理者");
                            axum::Json(json!({ "token": "first", "user": user_json() }))
                        },
                    ),
                ),
        )
        .await;
        let client = ApiClient::new().unwrap();
        assert!(client.setup_status(&url).await.unwrap().needs_setup);
        let resp = client
            .setup_admin(
                &url,
                &CreateAdminRequest {
                    username: "admin".into(),
                    display_name: "管理者".into(),
                    password: "0123456789".into(),
                },
            )
            .await
            .unwrap();
        assert_eq!(resp.token, "first");
    }

    fn managed_user_json(version: i32) -> serde_json::Value {
        json!({
            "id": "0196f5a0-0000-7000-8000-000000000002",
            "username": "editor1",
            "display_name": "陳編輯",
            "role": "editor",
            "role_name": "可編輯者",
            "is_active": true,
            "is_locked": false,
            "locked_until": null,
            "must_change_password": true,
            "last_login_at": null,
            "created_at": "2026-11-13T01:00:00Z",
            "updated_at": "2026-11-13T01:00:00Z",
            "version": version,
        })
    }

    #[tokio::test]
    async fn user_management_endpoints() {
        use axum::extract::{Path, RawQuery};
        use axum::routing::patch;

        let url = serve(
            Router::new()
                .route(
                    "/roles",
                    get(|| async {
                        axum::Json(json!([{
                            "id": "0196f5a0-0000-7000-8000-0000000000aa",
                            "code": "editor",
                            "name": "可編輯者",
                            "permissions": ["record.read"],
                        }]))
                    }),
                )
                .route(
                    "/users",
                    get(|| async { axum::Json(json!([managed_user_json(1)])) }).post(
                        |headers: AxumHeaders, axum::Json(body): axum::Json<serde_json::Value>| async move {
                            assert_eq!(bearer(&headers), "Bearer tok");
                            assert_eq!(body["username"], "editor1");
                            assert_eq!(body["password"], "0123456789");
                            (AxumStatus::CREATED, axum::Json(managed_user_json(1)))
                        },
                    ),
                )
                .route(
                    "/users/{id}",
                    patch(
                        |Path(id): Path<String>, axum::Json(body): axum::Json<serde_json::Value>| async move {
                            assert!(id.ends_with("0002"));
                            assert_eq!(
                                body,
                                json!({ "version": 1, "display_name": null, "role": null, "is_active": false })
                            );
                            (
                                AxumStatus::CONFLICT,
                                axum::Json(json!({ "code": "CONFLICT", "message": "此資料已被其他人修改，請重新載入" })),
                            )
                        },
                    )
                    .delete(|RawQuery(query): RawQuery| async move {
                        assert_eq!(query.as_deref(), Some("version=3"));
                        AxumStatus::NO_CONTENT
                    }),
                )
                .route(
                    "/users/{id}/reset-password",
                    post(|axum::Json(body): axum::Json<serde_json::Value>| async move {
                        assert_eq!(body, json!({ "new_password": "temp-password" }));
                        axum::Json(managed_user_json(2))
                    }),
                )
                .route(
                    "/users/{id}/unlock",
                    post(|| async { axum::Json(managed_user_json(3)) }),
                ),
        )
        .await;
        let client = ApiClient::new().unwrap();
        let id: Id = "0196f5a0-0000-7000-8000-000000000002".parse().unwrap();

        assert_eq!(
            client.list_roles(&url, "tok").await.unwrap()[0].code,
            "editor"
        );
        assert_eq!(client.list_users(&url, "tok").await.unwrap()[0].version, 1);
        let created = client
            .create_user(
                &url,
                "tok",
                &CreateUserRequest {
                    username: "editor1".into(),
                    display_name: "陳編輯".into(),
                    role: "editor".into(),
                    password: "0123456789".into(),
                },
            )
            .await
            .unwrap();
        assert!(created.must_change_password);

        let err = client
            .update_user(
                &url,
                "tok",
                id,
                &UpdateUserRequest {
                    version: 1,
                    display_name: None,
                    role: None,
                    is_active: Some(false),
                },
            )
            .await
            .unwrap_err();
        let ClientError::Api(api) = err else {
            panic!("應該是 ApiError：{err:?}");
        };
        assert_eq!(api.code, ErrorCode::Conflict);

        let reset = client
            .reset_user_password(
                &url,
                "tok",
                id,
                &ResetPasswordRequest {
                    new_password: "temp-password".into(),
                },
            )
            .await
            .unwrap();
        assert_eq!(reset.version, 2);
        assert_eq!(
            client.unlock_user(&url, "tok", id).await.unwrap().version,
            3
        );
        client
            .delete_user(&url, "tok", id, &DeleteUserQuery { version: 3 })
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn audit_log_query_string() {
        use axum::extract::RawQuery;
        use time::macros::datetime;

        let url = serve(Router::new().route(
            "/audit-logs",
            get(|RawQuery(query): RawQuery| async move {
                // 只帶有值的條件；時間的 `+` 必須編碼成 `%2B`，否則伺服器會當成空白
                assert_eq!(
                    query.as_deref(),
                    Some("from=2026-11-01T00%3A00%3A00%2B08%3A00&action=login&page=2&page_size=20")
                );
                axum::Json(json!({ "items": [], "total": 0, "page": 2, "page_size": 20 }))
            }),
        ))
        .await;

        let query = AuditLogQuery {
            from: Some(datetime!(2026-11-01 00:00 +8)),
            action: Some("login".into()),
            page: Some(2),
            page_size: Some(20),
            ..Default::default()
        };
        let page = ApiClient::new()
            .unwrap()
            .list_audit_logs(&url, "tok", &query)
            .await
            .unwrap();
        assert_eq!((page.page, page.page_size), (2, 20));
    }

    fn entity_json() -> serde_json::Value {
        json!({
            "id": "0196f5a0-0000-7000-8000-000000000100",
            "key": "repair",
            "label": "維修單",
            "icon": null,
            "kind": "master",
            "is_system": false,
            "storage": "custom_records",
            "parent_entity_id": null,
            "numbering_pattern": null,
            "title_template": null,
            "sort_order": 0,
            "is_active": true,
            "created_at": "2026-11-13T01:00:00Z",
            "updated_at": "2026-11-13T01:00:00Z",
            "version": 1,
        })
    }

    fn field_json(field_key: &str, version: i32) -> serde_json::Value {
        json!({
            "id": "0196f5a0-0000-7000-8000-000000000101",
            "entity_id": "0196f5a0-0000-7000-8000-000000000100",
            "field_key": field_key,
            "label": "欄位",
            "data_type": "text",
            "is_system": false,
            "storage": "json",
            "is_required": false,
            "is_visible": true,
            "is_readonly": false,
            "is_searchable": false,
            "is_unique": false,
            "default_value": null,
            "validation": null,
            "options": null,
            "decimal_places": null,
            "relation_entity_id": null,
            "formula": null,
            "formula_result_type": null,
            "ui_widget": null,
            "column_width": null,
            "help_text": null,
            "sort_order": 0,
            "is_active": true,
            "created_at": "2026-11-13T01:00:00Z",
            "updated_at": "2026-11-13T01:00:00Z",
            "version": version,
        })
    }

    #[tokio::test]
    async fn schema_endpoints() {
        use axum::extract::Path;
        use axum::routing::{patch, put};
        use hammer_shared::schema::{DataType, EntityKind, FieldOrderItem};

        let url = serve(
            Router::new()
                .route(
                    "/entities",
                    get(|| async { axum::Json(json!([entity_json()])) }).post(
                        |axum::Json(body): axum::Json<serde_json::Value>| async move {
                            assert_eq!(body["key"], "repair");
                            assert_eq!(body["kind"], "master");
                            (
                                AxumStatus::CREATED,
                                axum::Json(json!({ "entity": entity_json(), "fields": [] })),
                            )
                        },
                    ),
                )
                .route(
                    "/entities/{key}",
                    get(|Path(key): Path<String>| async move {
                        assert_eq!(key, "repair");
                        axum::Json(
                            json!({ "entity": entity_json(), "fields": [field_json("name", 1)] }),
                        )
                    }),
                )
                .route(
                    "/entities/{key}/fields",
                    post(
                        |axum::Json(body): axum::Json<serde_json::Value>| async move {
                            assert_eq!(body["data_type"], "integer");
                            (AxumStatus::CREATED, axum::Json(field_json("qty", 1)))
                        },
                    ),
                )
                .route(
                    "/entities/{key}/fields/order",
                    put(
                        |axum::Json(body): axum::Json<serde_json::Value>| async move {
                            assert_eq!(
                                body,
                                json!({ "fields": [{ "field_key": "name", "version": 1 }] })
                            );
                            axum::Json(json!([field_json("name", 2)]))
                        },
                    )
                    // 代碼為 `order` 的欄位：伺服器的 PATCH 轉給修改欄位
                    .patch(
                        |axum::Json(body): axum::Json<serde_json::Value>| async move {
                            assert_eq!(body["version"], 4);
                            axum::Json(field_json("order", 5))
                        },
                    ),
                )
                .route(
                    "/entities/{key}/fields/{field_key}",
                    patch(|Path((_, field_key)): Path<(String, String)>| async move {
                        axum::Json(field_json(&field_key, 2))
                    }),
                )
                .route(
                    "/entities/{key}/fields/{field_key}/deactivate",
                    post(
                        |axum::Json(body): axum::Json<serde_json::Value>| async move {
                            assert_eq!(body, json!({ "version": 2 }));
                            let mut f = field_json("name", 3);
                            f["is_active"] = json!(false);
                            axum::Json(f)
                        },
                    ),
                )
                .route(
                    "/entities/{key}/fields/{field_key}/change-type",
                    post(|| async {
                        (
                            AxumStatus::UNPROCESSABLE_ENTITY,
                            axum::Json(json!({
                                "code": "VALIDATION",
                                "message": "有 1 筆資料無法轉換",
                                "details": { "field": "data_type", "total": 1, "records": [] },
                            })),
                        )
                    }),
                ),
        )
        .await;
        let client = ApiClient::new().unwrap();

        assert_eq!(
            client
                .list_entities(&url, "tok", &ListEntitiesQuery::default())
                .await
                .unwrap()[0]
                .key,
            "repair"
        );
        let detail = client.get_entity(&url, "tok", "repair").await.unwrap();
        assert_eq!(detail.fields[0].field_key, "name");
        let created = client
            .create_entity(
                &url,
                "tok",
                &CreateEntityRequest {
                    key: "repair".into(),
                    label: "維修單".into(),
                    icon: None,
                    kind: EntityKind::Master,
                    parent_entity_id: None,
                    numbering_pattern: None,
                    title_template: None,
                    sort_order: 0,
                },
            )
            .await
            .unwrap();
        assert!(created.fields.is_empty());

        let field: CreateFieldRequest = serde_json::from_value(json!({
            "field_key": "qty", "label": "數量", "data_type": "integer",
        }))
        .unwrap();
        assert_eq!(
            client
                .create_field(&url, "tok", "repair", &field)
                .await
                .unwrap()
                .field_key,
            "qty"
        );

        let mut update: UpdateFieldRequest = serde_json::from_value(json!({
            "version": 4, "label": "排序", "data_type": "text",
            "is_required": false, "is_visible": true, "is_readonly": false,
            "is_searchable": false, "is_unique": false, "default_value": null,
            "validation": null, "options": null, "decimal_places": null,
            "ui_widget": null, "column_width": null, "help_text": null,
            "sort_order": 0, "is_active": true,
        }))
        .unwrap();
        assert_eq!(
            client
                .update_field(&url, "tok", "repair", "order", &update)
                .await
                .unwrap()
                .version,
            5
        );
        update.version = 1;
        assert_eq!(
            client
                .update_field(&url, "tok", "repair", "name", &update)
                .await
                .unwrap()
                .version,
            2
        );

        let deactivated = client
            .deactivate_field(
                &url,
                "tok",
                "repair",
                "name",
                &DeactivateFieldRequest { version: 2 },
            )
            .await
            .unwrap();
        assert!(!deactivated.is_active);

        let reordered = client
            .reorder_fields(
                &url,
                "tok",
                "repair",
                &ReorderFieldsRequest {
                    fields: vec![FieldOrderItem {
                        field_key: "name".into(),
                        version: 1,
                    }],
                },
            )
            .await
            .unwrap();
        assert_eq!(reordered[0].version, 2);

        let err = client
            .change_field_type(
                &url,
                "tok",
                "repair",
                "name",
                &ChangeFieldTypeRequest {
                    version: 3,
                    data_type: DataType::Integer,
                    decimal_places: None,
                    options: None,
                    validation: None,
                },
            )
            .await
            .unwrap_err();
        let ClientError::Api(api) = err else {
            panic!("應該是 ApiError：{err:?}");
        };
        assert_eq!(api.code, ErrorCode::Validation);
        assert_eq!(api.details.unwrap()["field"], "data_type");

        // 不合法的代碼不送出請求
        assert!(matches!(
            client.get_entity(&url, "tok", "../users").await,
            Err(ClientError::InvalidInput { .. })
        ));
    }

    #[tokio::test]
    async fn entity_management_endpoints() {
        use axum::extract::{Path, RawQuery};
        use axum::routing::put;

        let url = serve(
            Router::new()
                .route(
                    "/entities",
                    get(|RawQuery(query): RawQuery| async move {
                        let mut e = entity_json();
                        if query.as_deref() == Some("include_inactive=true") {
                            e["is_active"] = json!(false);
                        } else {
                            assert_eq!(query.as_deref(), Some("include_inactive=false"));
                        }
                        axum::Json(json!([e]))
                    }),
                )
                .route(
                    "/entities/order",
                    put(
                        |axum::Json(body): axum::Json<serde_json::Value>| async move {
                            assert_eq!(
                                body,
                                json!({ "entities": [{ "key": "repair", "version": 1, "sort_order": 3 }] })
                            );
                            let mut e = entity_json();
                            e["sort_order"] = json!(3);
                            axum::Json(json!([e]))
                        },
                    ),
                )
                .route(
                    "/entities/{key}",
                    axum::routing::patch(
                        |Path(key): Path<String>,
                         axum::Json(body): axum::Json<serde_json::Value>| async move {
                            assert_eq!(key, "repair");
                            assert_eq!(body["numbering_pattern"], "RO{yyyyMM}-{0000}");
                            assert_eq!(body["is_active"], false);
                            let mut e = entity_json();
                            e["version"] = json!(2);
                            axum::Json(json!({ "entity": e, "fields": [] }))
                        },
                    )
                    .delete(
                        |Path(key): Path<String>, RawQuery(query): RawQuery| async move {
                            assert_eq!(key, "repair");
                            assert_eq!(query.as_deref(), Some("version=2"));
                            AxumStatus::NO_CONTENT
                        },
                    ),
                )
                .route(
                    "/entities/{key}/recalculate",
                    post(|Path(key): Path<String>| async move {
                        assert_eq!(key, "repair");
                        axum::Json(json!({ "processed": 12, "changed": 3, "with_errors": 1 }))
                    }),
                ),
        )
        .await;
        let client = ApiClient::new().unwrap();

        let active = client
            .list_entities(&url, "tok", &ListEntitiesQuery::default())
            .await
            .unwrap();
        assert!(active[0].is_active);
        let all = client
            .list_entities(
                &url,
                "tok",
                &ListEntitiesQuery {
                    include_inactive: true,
                },
            )
            .await
            .unwrap();
        assert!(!all[0].is_active);

        let update: UpdateEntityRequest = serde_json::from_value(json!({
            "version": 1, "label": "維修單", "icon": null,
            "numbering_pattern": "RO{yyyyMM}-{0000}", "title_template": null,
            "sort_order": 0, "is_active": false,
        }))
        .unwrap();
        let updated = client
            .update_entity(&url, "tok", "repair", &update)
            .await
            .unwrap();
        assert_eq!(updated.entity.version, 2);

        let reordered = client
            .reorder_entities(
                &url,
                "tok",
                &ReorderEntitiesRequest {
                    entities: vec![hammer_shared::schema::EntityOrderItem {
                        key: "repair".into(),
                        version: 1,
                        sort_order: 3,
                    }],
                },
            )
            .await
            .unwrap();
        assert_eq!(reordered[0].sort_order, 3);

        client
            .delete_entity(&url, "tok", "repair", &DeleteEntityQuery { version: 2 })
            .await
            .unwrap();
        // 不合法的代碼不送出請求
        assert!(matches!(
            client
                .delete_entity(&url, "tok", "../x", &DeleteEntityQuery { version: 1 })
                .await,
            Err(ClientError::InvalidInput { .. })
        ));

        let result = client
            .recalculate_entity(&url, "tok", "repair")
            .await
            .unwrap();
        assert_eq!(
            result,
            RecalculateResult {
                processed: 12,
                changed: 3,
                with_errors: 1,
            }
        );
        assert!(matches!(
            client.recalculate_entity(&url, "tok", "../x").await,
            Err(ClientError::InvalidInput { .. })
        ));
    }

    fn layout_json(version: i32) -> serde_json::Value {
        json!({
            "id": "0196f5a0-0000-7000-8000-000000000301",
            "user_id": "0196f5a0-0000-7000-8000-000000000001",
            "name": "我的版面",
            "config": { "columns": [{ "field_key": "qty", "width": 120 }], "sort": null, "filters": [] },
            "is_default": true,
            "created_at": "2026-11-13T01:00:00Z",
            "updated_at": "2026-11-13T01:00:00Z",
            "version": version,
        })
    }

    fn record_json(version: i32) -> serde_json::Value {
        json!({
            "id": "0196f5a0-0000-7000-8000-000000000201",
            "title": "冷氣",
            "data": { "name": "冷氣", "price": "0.10" },
            "created_at": "2026-11-13T01:00:00Z",
            "created_by": null,
            "updated_at": "2026-11-13T01:00:00Z",
            "updated_by": null,
            "version": version,
        })
    }

    #[tokio::test]
    async fn inventory_query_endpoints() {
        use axum::extract::{Path, RawQuery};

        let product_id: Id = "0196f5a0-0000-7000-8000-000000000601".parse().unwrap();
        let url = serve(
            Router::new()
                .route(
                    "/inventory/balances",
                    get(|RawQuery(query): RawQuery| async move {
                        // 沒有指定的參數不送出
                        assert_eq!(query.as_deref(), Some("q=%E8%9E%BA&below_safety_stock=true&page=1"));
                        axum::Json(json!({
                            "items": [{
                                "product_id": "0196f5a0-0000-7000-8000-000000000601",
                                "code": "A001", "name": "螺絲", "spec": null, "unit_name": "個",
                                "is_active": true, "qty_on_hand": "3.0000", "stock_value_twd": "30.00",
                                "safety_stock": "10.0000", "below_safety_stock": true,
                            }],
                            "total": 1, "page": 1, "page_size": 50,
                        }))
                    }),
                )
                .route(
                    "/inventory/products/{product_id}/lots",
                    get(|Path(id): Path<String>, RawQuery(query): RawQuery| async move {
                        assert_eq!(id, "0196f5a0-0000-7000-8000-000000000601");
                        assert_eq!(query.as_deref(), Some("include_empty=true"));
                        axum::Json(json!({
                            "items": [{
                                "id": "0196f5a0-0000-7000-8000-000000000701",
                                "lot_no": "L-01", "received_date": "2026-11-17",
                                "source_type": "stock_adjustment_line",
                                "source_doc_id": "0196f5a0-0000-7000-8000-000000000501",
                                "source_record_no": "ADJ202611-0001",
                                "qty_in": "5.0000", "qty_remaining": "3.0000", "unit_cost_twd": "10.0000",
                            }],
                            "total": 1, "page": 1, "page_size": 50,
                        }))
                    }),
                )
                .route(
                    "/inventory/transactions",
                    get(|RawQuery(query): RawQuery| async move {
                        assert_eq!(
                            query.as_deref(),
                            Some("product_id=0196f5a0-0000-7000-8000-000000000601")
                        );
                        axum::Json(json!({ "items": [], "total": 0, "page": 1, "page_size": 50 }))
                    }),
                ),
        )
        .await;
        let client = ApiClient::new().unwrap();

        let balances = client
            .list_stock_balances(
                &url,
                "tok",
                &StockBalanceQuery {
                    q: Some("螺".into()),
                    below_safety_stock: Some("true".into()),
                    page: Some(1),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert!(balances.items[0].below_safety_stock);
        assert_eq!(balances.items[0].qty_on_hand.to_string(), "3.0000");

        let lots = client
            .list_product_lots(
                &url,
                "tok",
                product_id,
                &LotQuery {
                    include_empty: Some("true".into()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(lots.items[0].source_record_no, "ADJ202611-0001");

        let txns = client
            .list_inventory_transactions(
                &url,
                "tok",
                &TransactionQuery {
                    product_id: Some(product_id.to_string()),
                    ..Default::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(txns.total, 0);
    }

    #[tokio::test]
    async fn allocation_preview_endpoint() {
        use hammer_shared::inventory::AllocationPreviewLineInput;

        let url = serve(Router::new().route(
            "/inventory/allocation-preview",
            post(
                |axum::Json(body): axum::Json<serde_json::Value>| async move {
                    assert_eq!(
                        body,
                        json!({ "lines": [{
                        "product_id": "0196f5a0-0000-7000-8000-000000000601",
                        "qty": "12", "lot_no": null,
                    }] })
                    );
                    axum::Json(json!({ "lines": [{
                    "index": 0, "product_id": "0196f5a0-0000-7000-8000-000000000601",
                    "product_type": "goods", "qty": "12.0000",
                    "allocations": [{
                        "lot_id": "0196f5a0-0000-7000-8000-000000000701", "lot_no": "L-01",
                        "received_date": "2026-10-02", "qty": "5.0000", "unit_cost_twd": "80.0000",
                    }],
                    "shortage": "7.0000",
                }] }))
                },
            ),
        ))
        .await;
        let client = ApiClient::new().unwrap();
        let preview = client
            .allocation_preview(
                &url,
                "tok",
                &AllocationPreviewRequest {
                    lines: vec![AllocationPreviewLineInput {
                        product_id: Some("0196f5a0-0000-7000-8000-000000000601".into()),
                        qty: Some("12".into()),
                        lot_no: None,
                    }],
                },
            )
            .await
            .unwrap();
        assert_eq!(preview.lines[0].allocations[0].lot_no, "L-01");
        assert_eq!(preview.lines[0].shortage.to_string(), "7.0000");
    }

    #[tokio::test]
    async fn invoicing_query_endpoints() {
        use axum::extract::RawQuery;

        let url = serve(
            Router::new()
                .route(
                    "/invoicing/uninvoiced-shipments",
                    get(|RawQuery(q): RawQuery| async move {
                        assert_eq!(q.as_deref(), Some("customer_id=c1&page=1"));
                        axum::Json(json!({ "items": [], "total": 0, "page": 1, "page_size": 50 }))
                    }),
                )
                .route(
                    "/invoicing/links",
                    get(|RawQuery(q): RawQuery| async move {
                        assert_eq!(q.as_deref(), Some("entity_key=shipment&record_id=r1"));
                        axum::Json(json!([{
                            "entity_key": "sales_invoice",
                            "record_id": "0196f5a0-0000-7000-8000-000000000601",
                            "record_no": "SI202611-0001", "invoice_no": "AB12345678",
                            "invoice_date": "2026-11-19", "status": "posted",
                        }]))
                    }),
                ),
        )
        .await;
        let client = ApiClient::new().unwrap();
        let page = client
            .list_uninvoiced_shipments(
                &url,
                "tok",
                &UninvoicedShipmentQuery {
                    customer_id: Some("c1".into()),
                    page: Some(1),
                    ..UninvoicedShipmentQuery::default()
                },
            )
            .await
            .unwrap();
        assert_eq!(page.total, 0);
        let links = client
            .list_invoice_links(
                &url,
                "tok",
                &InvoiceLinkQuery {
                    entity_key: Some("shipment".into()),
                    record_id: Some("r1".into()),
                },
            )
            .await
            .unwrap();
        assert_eq!(links[0].invoice_no.as_deref(), Some("AB12345678"));
    }

    #[tokio::test]
    async fn document_action_endpoints() {
        use axum::extract::Path;

        let record_id: Id = "0196f5a0-0000-7000-8000-000000000201".parse().unwrap();
        let url = serve(
            Router::new()
                .route(
                    "/entities/{key}/records/{id}/post",
                    post(
                        |Path((key, id)): Path<(String, String)>,
                         axum::Json(body): axum::Json<serde_json::Value>| async move {
                            assert_eq!(key, "stock_adjustment");
                            assert_eq!(id, "0196f5a0-0000-7000-8000-000000000201");
                            // request 只有 version
                            assert_eq!(body, json!({ "version": 2 }));
                            axum::Json(record_json(3))
                        },
                    ),
                )
                .route(
                    "/entities/{key}/records/{id}/void",
                    post(|| async {
                        (
                            AxumStatus::UNPROCESSABLE_ENTITY,
                            axum::Json(json!({
                                "code": "STOCK_INSUFFICIENT",
                                "message": "庫存不足",
                                "details": { "items": [] },
                            })),
                        )
                    }),
                ),
        )
        .await;
        let client = ApiClient::new().unwrap();

        let posted = client
            .document_action(
                &url,
                "tok",
                "stock_adjustment",
                record_id,
                DocumentAction::Post,
                &DocumentActionRequest::new(2),
            )
            .await
            .unwrap();
        assert_eq!(posted.version, 3);

        let err = client
            .document_action(
                &url,
                "tok",
                "stock_adjustment",
                record_id,
                DocumentAction::Void,
                &DocumentActionRequest::new(3),
            )
            .await
            .unwrap_err();
        let ClientError::Api(api) = err else {
            panic!("應該是 ApiError：{err:?}");
        };
        assert_eq!(api.code, ErrorCode::StockInsufficient);
        assert_eq!(api.details, Some(json!({ "items": [] })));
    }

    #[tokio::test]
    async fn record_and_layout_endpoints() {
        use axum::extract::{Path, RawQuery};
        use axum::routing::patch;
        use hammer_shared::layout::LayoutConfig;
        use hammer_shared::record::{FilterOp, RecordFilter, RecordSort};

        let record_id: Id = "0196f5a0-0000-7000-8000-000000000201".parse().unwrap();
        let layout_id: Id = "0196f5a0-0000-7000-8000-000000000301".parse().unwrap();

        let url = serve(
            Router::new()
                .route(
                    "/entities/{key}/records",
                    get(|Path(key): Path<String>, RawQuery(query): RawQuery| async move {
                        assert_eq!(key, "repair");
                        // 動態的篩選參數名稱；值經過 URL 編碼
                        assert_eq!(
                            query.as_deref(),
                            Some(
                                "page=2&page_size=20&sort=-qty&filter.name.contains=%E8%BB%8A\
                                 &filter.tags.any=a&filter.tags.any=b"
                            )
                        );
                        axum::Json(json!({
                            "items": [{
                                "id": "0196f5a0-0000-7000-8000-000000000201",
                                "title": "0196f5a0",
                                "data": { "qty": 3, "price": "12.5000" },
                                "created_at": "2026-11-13T01:00:00Z",
                                "created_by": null,
                                "updated_at": "2026-11-13T01:00:00Z",
                                "updated_by": null,
                                "version": 1,
                            }],
                            "total": 21, "page": 2, "page_size": 20,
                        }))
                    })
                    .post(|axum::Json(body): axum::Json<serde_json::Value>| async move {
                        assert_eq!(body, json!({ "data": { "name": "冷氣", "price": "0.1" } }));
                        (AxumStatus::CREATED, axum::Json(record_json(1)))
                    }),
                )
                .route(
                    "/entities/{key}/records/{id}",
                    get(|Path((_, id)): Path<(String, String)>| async move {
                        assert_eq!(id, "0196f5a0-0000-7000-8000-000000000201");
                        axum::Json(record_json(3))
                    })
                    .patch(|axum::Json(body): axum::Json<serde_json::Value>| async move {
                        // 小數以字串送出；null 代表清空
                        assert_eq!(
                            body,
                            json!({ "version": 3, "data": { "price": "0.3", "memo": null } })
                        );
                        axum::Json(record_json(4))
                    })
                    .delete(
                        |Path((_, id)): Path<(String, String)>, RawQuery(query): RawQuery| async move {
                            assert_eq!(id, "0196f5a0-0000-7000-8000-000000000201");
                            assert_eq!(query.as_deref(), Some("version=4"));
                            AxumStatus::NO_CONTENT
                        },
                    ),
                )
                .route(
                    "/entities/{key}/layouts",
                    get(|| async { axum::Json(json!([layout_json(1)])) }).post(
                        |axum::Json(body): axum::Json<serde_json::Value>| async move {
                            assert_eq!(body["name"], "我的版面");
                            assert_eq!(body["is_global"], false);
                            assert_eq!(body["is_default"], true);
                            (AxumStatus::CREATED, axum::Json(layout_json(1)))
                        },
                    ),
                )
                .route(
                    "/entities/{key}/layouts/default",
                    get(|| async {
                        axum::Json(json!({
                            "source": "generated",
                            "layout": null,
                            "config": { "columns": [], "sort": null, "filters": [] },
                        }))
                    }),
                )
                .route(
                    "/entities/{key}/layouts/{id}",
                    patch(|axum::Json(body): axum::Json<serde_json::Value>| async move {
                        assert_eq!(body["version"], 1);
                        assert_eq!(body["config"]["sort"], json!({ "field_key": "qty", "descending": true }));
                        axum::Json(layout_json(2))
                    })
                    .delete(|RawQuery(query): RawQuery| async move {
                        assert_eq!(query.as_deref(), Some("version=2"));
                        AxumStatus::NO_CONTENT
                    }),
                ),
        )
        .await;
        let client = ApiClient::new().unwrap();

        let query = RecordListQuery {
            page: Some(2),
            page_size: Some(20),
            sort: Some(RecordSort {
                field_key: "qty".into(),
                descending: true,
            }),
            filters: vec![
                RecordFilter {
                    field_key: "name".into(),
                    op: FilterOp::Contains,
                    value: "車".into(),
                },
                RecordFilter {
                    field_key: "tags".into(),
                    op: FilterOp::Any,
                    value: "a".into(),
                },
                RecordFilter {
                    field_key: "tags".into(),
                    op: FilterOp::Any,
                    value: "b".into(),
                },
            ],
        };
        let page = client
            .list_records(&url, "tok", "repair", &query)
            .await
            .unwrap();
        assert_eq!(
            (page.total, page.items[0].data["price"].clone()),
            (21, json!("12.5000"))
        );

        let fetched = client
            .get_record(&url, "tok", "repair", record_id)
            .await
            .unwrap();
        assert_eq!(fetched.version, 3);

        let created = client
            .create_record(
                &url,
                "tok",
                "repair",
                &CreateRecordRequest {
                    data: json!({ "name": "冷氣", "price": "0.1" })
                        .as_object()
                        .unwrap()
                        .clone(),
                    lines: Default::default(),
                },
            )
            .await
            .unwrap();
        assert_eq!(created.version, 1);

        let updated = client
            .update_record(
                &url,
                "tok",
                "repair",
                record_id,
                &UpdateRecordRequest {
                    version: 3,
                    data: json!({ "price": "0.3", "memo": null })
                        .as_object()
                        .unwrap()
                        .clone(),
                    lines: Default::default(),
                },
            )
            .await
            .unwrap();
        assert_eq!(updated.version, 4);

        client
            .delete_record(
                &url,
                "tok",
                "repair",
                record_id,
                &DeleteRecordQuery { version: 4 },
            )
            .await
            .unwrap();

        assert_eq!(
            client.list_layouts(&url, "tok", "repair").await.unwrap()[0].version,
            1
        );
        let default = client
            .get_default_layout(&url, "tok", "repair")
            .await
            .unwrap();
        assert!(default.layout.is_none());

        let created = client
            .create_layout(
                &url,
                "tok",
                "repair",
                &CreateViewLayoutRequest {
                    name: "我的版面".into(),
                    is_global: false,
                    config: LayoutConfig::default(),
                    is_default: true,
                },
            )
            .await
            .unwrap();
        assert_eq!(created.config.columns[0].width, Some(120));

        let updated = client
            .update_layout(
                &url,
                "tok",
                "repair",
                layout_id,
                &UpdateViewLayoutRequest {
                    version: 1,
                    name: None,
                    config: Some(LayoutConfig {
                        sort: Some(RecordSort {
                            field_key: "qty".into(),
                            descending: true,
                        }),
                        ..LayoutConfig::default()
                    }),
                    is_default: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(updated.version, 2);

        client
            .delete_layout(
                &url,
                "tok",
                "repair",
                layout_id,
                &DeleteViewLayoutQuery { version: 2 },
            )
            .await
            .unwrap();

        // 不合法的代碼不送出請求
        assert!(matches!(
            client
                .list_records(&url, "tok", "Repair", &RecordListQuery::default())
                .await,
            Err(ClientError::InvalidInput { .. })
        ));
    }

    #[tokio::test]
    async fn lookup_endpoint() {
        use axum::extract::{Path, RawQuery};

        let url = serve(Router::new().route(
            "/entities/{key}/records/lookup",
            get(
                |Path(key): Path<String>, RawQuery(query): RawQuery| async move {
                    assert_eq!(key, "equipment");
                    axum::Json(json!([{
                        "id": "0196f5a0-0000-7000-8000-000000000201",
                        "title": format!("{key}?{}", query.unwrap_or_default()),
                    }]))
                },
            ),
        ))
        .await;
        let client = ApiClient::new().unwrap();
        let query = LookupQuery {
            q: Some("台北 pump".into()),
            limit: Some(20),
        };
        let items = client
            .lookup_records(&url, "tok", "equipment", &query)
            .await
            .unwrap();
        assert_eq!(items.len(), 1);
        assert_eq!(
            items[0].title,
            "equipment?q=%E5%8F%B0%E5%8C%97+pump&limit=20"
        );
        // 沒有關鍵字時不帶 q
        let items = client
            .lookup_records(&url, "tok", "equipment", &LookupQuery::default())
            .await
            .unwrap();
        assert_eq!(items[0].title, "equipment?");
    }

    fn attachment_json(file_name: &str, size: usize) -> serde_json::Value {
        json!({
            "id": "0196f5a0-0000-7000-8000-000000000401",
            "file_name": file_name,
            "mime_type": "text/plain",
            "size_bytes": size,
            "sha256": "abc",
            "created_at": "2026-11-16T01:00:00Z",
        })
    }

    #[tokio::test]
    async fn upload_sends_multipart_and_reports_progress() {
        use axum::extract::Multipart;
        use std::sync::{Arc, Mutex};

        let url = serve(Router::new().route(
            "/attachments",
            post(
                |headers: AxumHeaders, mut multipart: Multipart| async move {
                    assert_eq!(bearer(&headers), "Bearer tok");
                    let field = multipart.next_field().await.unwrap().unwrap();
                    assert_eq!(field.name(), Some(UPLOAD_FIELD));
                    let file_name = field.file_name().unwrap().to_owned();
                    let mime = field.content_type().unwrap().to_owned();
                    let content = field.bytes().await.unwrap();
                    assert_eq!(mime, "text/plain");
                    assert!(content.iter().all(|&b| b == b'x'));
                    (
                        AxumStatus::CREATED,
                        axum::Json(attachment_json(&file_name, content.len())),
                    )
                },
            ),
        ))
        .await;
        let client = ApiClient::new().unwrap();
        let size = UPLOAD_CHUNK_BYTES * 2 + 10;
        let progress = Arc::new(Mutex::new(Vec::new()));
        let recorded = progress.clone();
        let file = UploadFile {
            file_name: "維修報告.txt".into(),
            mime_type: Some("text/plain".into()),
            size_bytes: size as u64,
            body: progress_body(std::io::Cursor::new(vec![b'x'; size]), move |sent| {
                recorded.lock().unwrap().push(sent);
            }),
        };
        let attachment = client.upload_attachment(&url, "tok", file).await.unwrap();
        assert_eq!(attachment.file_name, "維修報告.txt");
        assert_eq!(attachment.size_bytes, size as u64);
        assert_eq!(
            *progress.lock().unwrap(),
            [
                UPLOAD_CHUNK_BYTES as u64,
                UPLOAD_CHUNK_BYTES as u64 * 2,
                size as u64
            ]
        );
    }

    #[tokio::test]
    async fn upload_too_large_and_expired_session() {
        use axum::response::IntoResponse;
        let url = serve(Router::new().route(
            "/attachments",
            post(|headers: AxumHeaders| async move {
                if bearer(&headers) == "Bearer expired" {
                    return unauthorized("登入已逾時").into_response();
                }
                (
                    AxumStatus::PAYLOAD_TOO_LARGE,
                    axum::Json(json!({
                        "code": "PAYLOAD_TOO_LARGE",
                        "message": "檔案超過大小上限 20 MB",
                    })),
                )
                    .into_response()
            }),
        ))
        .await;
        let client = ApiClient::new().unwrap();
        let file = || UploadFile {
            file_name: "a.bin".into(),
            mime_type: None,
            size_bytes: 3,
            body: progress_body(std::io::Cursor::new(b"abc".to_vec()), |_| {}),
        };
        let err = client
            .upload_attachment(&url, "tok", file())
            .await
            .unwrap_err();
        let ClientError::Api(api) = err else {
            panic!("應該是 ApiError：{err:?}");
        };
        assert_eq!(api.code, ErrorCode::PayloadTooLarge);
        assert_eq!(api.message, "檔案超過大小上限 20 MB");
        assert!(matches!(
            client.upload_attachment(&url, "expired", file()).await,
            Err(ClientError::SessionExpired { .. })
        ));
    }

    #[tokio::test]
    async fn download_returns_content_or_error() {
        use axum::extract::Path;
        use axum::response::IntoResponse;

        let ok_id: Id = "0196f5a0-0000-7000-8000-000000000401".parse().unwrap();
        let url = serve(Router::new().route(
            "/attachments/{id}",
            get(move |Path(id): Path<Id>| async move {
                if id == ok_id {
                    "中文內容".as_bytes().to_vec().into_response()
                } else {
                    (
                        AxumStatus::NOT_FOUND,
                        axum::Json(json!({ "code": "NOT_FOUND", "message": "找不到此附件" })),
                    )
                        .into_response()
                }
            }),
        ))
        .await;
        let client = ApiClient::new().unwrap();
        assert_eq!(
            client
                .download_attachment(&url, "tok", ok_id)
                .await
                .unwrap(),
            "中文內容".as_bytes()
        );
        let err = client
            .download_attachment(&url, "tok", Id::nil())
            .await
            .unwrap_err();
        assert!(
            matches!(&err, ClientError::Api(api) if api.code == ErrorCode::NotFound),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn list_currencies_sends_include_inactive() {
        let url = serve(Router::new().route(
            "/currencies",
            get(
                |axum::extract::RawQuery(query): axum::extract::RawQuery| async move {
                    let inactive = query.as_deref() == Some("include_inactive=true");
                    axum::Json(json!([{
                        "id": "0190a000-0000-7000-8000-000000000001",
                        "code": if inactive { "XXX" } else { "TWD" },
                        "name": "新台幣", "symbol": "NT$", "decimal_places": 0,
                        "is_base": !inactive, "is_active": !inactive,
                        "created_at": "2026-11-17T00:00:00Z",
                        "updated_at": "2026-11-17T00:00:00Z", "version": 1,
                    }]))
                },
            ),
        ))
        .await;
        let client = ApiClient::new().unwrap();

        let active = client
            .list_currencies(&url, "tok", &CurrencyListQuery::default())
            .await
            .unwrap();
        assert_eq!(active[0].code, "TWD");
        assert!(active[0].is_base);
        let all = client
            .list_currencies(
                &url,
                "tok",
                &CurrencyListQuery {
                    include_inactive: Some(true),
                },
            )
            .await
            .unwrap();
        assert_eq!(all[0].code, "XXX");
        assert!(!all[0].is_active);
    }

    fn exchange_rate_json(version: i32) -> serde_json::Value {
        json!({
            "id": "0190a000-0000-7000-8000-0000000000e1",
            "currency_code": "USD",
            "rate_date": "2026-11-17",
            "rate": "32.150000",
            "created_at": "2026-11-17T00:00:00Z",
            "updated_at": "2026-11-17T00:00:00Z",
            "version": version,
        })
    }

    #[tokio::test]
    async fn currency_management_endpoints() {
        use axum::extract::Path;
        use axum::routing::patch;

        let currency = |version: i32| {
            json!({
                "id": "0190a000-0000-7000-8000-000000000002",
                "code": "GBP", "name": "英鎊", "symbol": null, "decimal_places": 2,
                "is_base": false, "is_active": true,
                "created_at": "2026-11-17T00:00:00Z",
                "updated_at": "2026-11-17T00:00:00Z", "version": version,
            })
        };
        let url = serve(
            Router::new()
                .route(
                    "/currencies",
                    post(
                        move |headers: AxumHeaders, axum::Json(body): axum::Json<serde_json::Value>| async move {
                            assert_eq!(bearer(&headers), "Bearer tok");
                            assert_eq!(
                                body,
                                json!({ "code": "gbp", "name": "英鎊", "symbol": null, "decimal_places": 2 })
                            );
                            (AxumStatus::CREATED, axum::Json(currency(1)))
                        },
                    ),
                )
                .route(
                    "/currencies/{code}",
                    patch(
                        move |Path(code): Path<String>, axum::Json(body): axum::Json<serde_json::Value>| async move {
                            assert_eq!(code, "GBP");
                            assert_eq!(
                                body,
                                json!({ "version": 1, "name": null, "symbol": "", "decimal_places": null, "is_active": false })
                            );
                            axum::Json(currency(2))
                        },
                    ),
                ),
        )
        .await;
        let client = ApiClient::new().unwrap();

        let created = client
            .create_currency(
                &url,
                "tok",
                &CreateCurrencyRequest {
                    code: "gbp".into(),
                    name: "英鎊".into(),
                    symbol: None,
                    decimal_places: 2,
                },
            )
            .await
            .unwrap();
        assert_eq!(created.code, "GBP");
        let req = UpdateCurrencyRequest {
            version: 1,
            name: None,
            symbol: Some(String::new()),
            decimal_places: None,
            is_active: Some(false),
        };
        let updated = client
            .update_currency(&url, "tok", "GBP", &req)
            .await
            .unwrap();
        assert_eq!(updated.version, 2);

        // 代碼含其他字元時不送出請求，避免改變路徑
        let err = client
            .update_currency(&url, "tok", "../x", &req)
            .await
            .unwrap_err();
        assert!(matches!(err, ClientError::InvalidInput { .. }), "{err:?}");
    }

    #[tokio::test]
    async fn effective_rate_and_tax_rate_endpoints() {
        use axum::extract::RawQuery;

        let url = serve(
            Router::new()
                .route(
                    "/exchange-rates/effective",
                    get(|RawQuery(query): RawQuery| async move {
                        assert_eq!(query.as_deref(), Some("currency_code=USD&date=2026-11-18"));
                        axum::Json(json!({
                            "currency_code": "USD", "date": "2026-11-18",
                            "rate": "32.150000", "rate_date": "2026-11-15",
                        }))
                    }),
                )
                .route(
                    "/tax-rates",
                    get(|| async {
                        axum::Json(json!([
                            { "code": "taxable", "name": "應稅", "rate": "0.0500" },
                            { "code": "zero_rated", "name": "零稅率", "rate": "0.0000" },
                        ]))
                    }),
                ),
        )
        .await;
        let client = ApiClient::new().unwrap();

        let rate = client
            .effective_exchange_rate(
                &url,
                "tok",
                &EffectiveRateQuery {
                    currency_code: "USD".into(),
                    date: time::macros::date!(2026 - 11 - 18),
                },
            )
            .await
            .unwrap();
        assert_eq!(rate.rate.unwrap().to_string(), "32.150000");
        assert_eq!(rate.rate_date, Some(time::macros::date!(2026 - 11 - 15)));

        let taxes = client.list_tax_rates(&url, "tok").await.unwrap();
        assert_eq!(taxes.len(), 2);
        assert_eq!(taxes[0].code, "taxable");
    }

    #[tokio::test]
    async fn confirm_and_close_endpoints() {
        use axum::extract::Path;

        let record_id: Id = "0196f5a0-0000-7000-8000-000000000201".parse().unwrap();
        let url = serve(Router::new().route(
            "/entities/{key}/records/{id}/{action}",
            post(
                |Path((key, _id, action)): Path<(String, String, String)>,
                 axum::Json(body): axum::Json<serde_json::Value>| async move {
                    assert_eq!(key, "purchase_order");
                    assert!(action == "confirm" || action == "close", "{action}");
                    let version = body["version"].as_i64().unwrap() as i32;
                    axum::Json(record_json(version + 1))
                },
            ),
        ))
        .await;
        let client = ApiClient::new().unwrap();
        for (action, version) in [(DocumentAction::Confirm, 1), (DocumentAction::Close, 2)] {
            let record = client
                .document_action(
                    &url,
                    "tok",
                    "purchase_order",
                    record_id,
                    action,
                    &DocumentActionRequest::new(version),
                )
                .await
                .unwrap();
            assert_eq!(record.version, version + 1);
        }
    }

    #[tokio::test]
    async fn exchange_rate_endpoints() {
        use axum::extract::{Path, RawQuery};
        use axum::routing::patch;

        let url = serve(
            Router::new()
                .route(
                    "/exchange-rates",
                    get(|RawQuery(query): RawQuery| async move {
                        // `None` 的條件（date_to）不放進 query string
                        assert_eq!(
                            query.as_deref(),
                            Some("currency_code=USD&date_from=2026-11-01&page=2&page_size=20")
                        );
                        axum::Json(json!({
                            "items": [exchange_rate_json(1)],
                            "page": 2, "page_size": 20, "total": 21,
                        }))
                    })
                    .post(|axum::Json(body): axum::Json<serde_json::Value>| async move {
                        // 匯率以字串送出，不轉成浮點數
                        assert_eq!(
                            body,
                            json!({ "currency_code": "USD", "rate_date": "2026-11-17", "rate": "32.15" })
                        );
                        (
                            AxumStatus::UNPROCESSABLE_ENTITY,
                            axum::Json(json!({
                                "code": "VALIDATION",
                                "message": "同一幣別同一天已有匯率",
                                "details": { "field": "rate_date" },
                            })),
                        )
                    }),
                )
                .route(
                    "/exchange-rates/{id}",
                    patch(
                        |Path(id): Path<String>, axum::Json(body): axum::Json<serde_json::Value>| async move {
                            assert!(id.ends_with("00e1"));
                            assert_eq!(body, json!({ "version": 1, "rate": "32.200000" }));
                            axum::Json(exchange_rate_json(2))
                        },
                    )
                    .delete(|RawQuery(query): RawQuery| async move {
                        assert_eq!(query.as_deref(), Some("version=2"));
                        AxumStatus::NO_CONTENT
                    }),
                ),
        )
        .await;
        let client = ApiClient::new().unwrap();
        let id: Id = "0190a000-0000-7000-8000-0000000000e1".parse().unwrap();

        let page = client
            .list_exchange_rates(
                &url,
                "tok",
                &ExchangeRateQuery {
                    currency_code: Some("USD".into()),
                    date_from: Some(time::macros::date!(2026 - 11 - 01)),
                    date_to: None,
                    page: Some(2),
                    page_size: Some(20),
                },
            )
            .await
            .unwrap();
        assert_eq!(page.total, 21);
        assert_eq!(page.items[0].rate.to_string(), "32.150000");

        let err = client
            .create_exchange_rate(
                &url,
                "tok",
                &CreateExchangeRateRequest {
                    currency_code: "USD".into(),
                    rate_date: time::macros::date!(2026 - 11 - 17),
                    rate: "32.15".parse().unwrap(),
                },
            )
            .await
            .unwrap_err();
        let ClientError::Api(api) = err else {
            panic!("應該是 ApiError：{err:?}");
        };
        assert_eq!(api.code, ErrorCode::Validation);

        let updated = client
            .update_exchange_rate(
                &url,
                "tok",
                id,
                &UpdateExchangeRateRequest {
                    version: 1,
                    rate: "32.200000".parse().unwrap(),
                },
            )
            .await
            .unwrap();
        assert_eq!(updated.version, 2);
        client
            .delete_exchange_rate(&url, "tok", id, &DeleteExchangeRateQuery { version: 2 })
            .await
            .unwrap();
    }
}

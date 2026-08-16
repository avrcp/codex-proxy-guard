use std::sync::Arc;

use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    middleware,
    response::{IntoResponse, Response},
    routing::{get, patch, post},
};
use chrono::Utc;
use proxy_guard_core::{
    BenchmarkRunSummary, CodexRegion, ManagedNodeState, NodeId, ProxyMode, SubscriptionId,
    SubscriptionSyncSummary, TaskResult,
};
use proxy_guard_network::SubscriptionUpdate;
use serde::Deserialize;
use tokio::sync::OwnedSemaphorePermit;

use super::{assets, auth, dto, response::AppError, state::ManagerOperation, state::ManagerState};

/// Run one blocking network/storage operation on the blocking pool.
async fn blocking<T, F>(f: F) -> Result<T, AppError>
where
    T: Send + 'static,
    F: FnOnce() -> anyhow::Result<T> + Send + 'static,
{
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|error| AppError::Internal(format!("manager task failed: {error}")))?
        .map_err(|error| AppError::Internal(error.to_string()))
}

async fn notify_managed_view(state: &Arc<ManagerState>) {
    let config = state.config.lock().await.clone();
    let message = match blocking(move || crate::managed_services::load_managed_view(&config)).await
    {
        Ok(view) => TaskResult::ManagerManagedViewUpdated(Ok(view)),
        Err(error) => TaskResult::ManagerManagedViewUpdated(Err(error.to_string())),
    };
    let _ = state.tx.send(message).await;
}

// ---------------------------------------------------------------------------
// Overview
// ---------------------------------------------------------------------------

async fn get_state(
    State(state): State<Arc<ManagerState>>,
) -> Result<Json<dto::StateDto>, AppError> {
    let config = state.config.lock().await.clone();
    let view_config = config.clone();
    let view = blocking(move || crate::managed_services::load_managed_view(&view_config)).await?;
    let manual = state.manual_selection.lock().await.clone();
    let selection = manual.clone().or_else(|| view.selected.clone());
    let managed = config.is_managed();
    let manual_active = manual.is_some();
    Ok(Json(dto::StateDto {
        mode: if managed { "managed" } else { "external" },
        subscription_name: view.subscription_name,
        regions: view.regions,
        auto_selected: view.selected,
        manual_selected: manual,
        selection,
        manual_active,
    }))
}

async fn get_operation(State(state): State<Arc<ManagerState>>) -> Json<dto::OperationDto> {
    let operation = state.operation.lock().await.clone();
    let last_benchmark = state.last_benchmark.lock().await.clone();
    let (key, subscription_id, started_at, message) = match operation {
        ManagerOperation::Idle => ("idle", None, None, None),
        ManagerOperation::Syncing { subscription_id } => {
            ("syncing", Some(subscription_id.to_string()), None, None)
        }
        ManagerOperation::Benchmarking { started_at } => {
            ("benchmarking", None, Some(started_at), None)
        }
        ManagerOperation::Failed { message } => ("failed", None, None, Some(message)),
    };
    Json(dto::OperationDto {
        state: key,
        subscription_id,
        started_at,
        message,
        last_benchmark,
    })
}

// ---------------------------------------------------------------------------
// Subscriptions
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct InspectRequest {
    pub url: String,
}

#[derive(Deserialize)]
pub struct CreateSubscriptionRequest {
    pub name: String,
    pub url: String,
}

#[derive(Deserialize, Default)]
pub struct UpdateSubscriptionRequest {
    pub name: Option<String>,
    pub url: Option<String>,
    pub enabled: Option<bool>,
}

async fn list_subscriptions(
    State(state): State<Arc<ManagerState>>,
) -> Result<Json<Vec<dto::SubscriptionDto>>, AppError> {
    let config = state.config.lock().await.clone();
    let active_id = crate::managed_services::configured_subscription(&config);
    let subscriptions = blocking(|| {
        crate::managed_services::subscription_service()?
            .list()
            .map_err(anyhow::Error::from)
    })
    .await?;
    Ok(Json(
        subscriptions
            .iter()
            .map(|stored| dto::subscription_dto(stored, active_id))
            .collect(),
    ))
}

async fn inspect_subscription(
    State(_state): State<Arc<ManagerState>>,
    Json(body): Json<InspectRequest>,
) -> Result<Json<dto::SubscriptionPreviewDto>, AppError> {
    let preview = blocking(move || {
        crate::managed_services::subscription_service()?
            .inspect(&body.url)
            .map_err(anyhow::Error::from)
    })
    .await?;
    Ok(Json(dto::preview_dto(&preview)))
}

async fn create_subscription(
    State(state): State<Arc<ManagerState>>,
    Json(body): Json<CreateSubscriptionRequest>,
) -> Result<Response, AppError> {
    let _permit = state
        .operation_permit
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::OperationBusy)?;
    let created = blocking(move || {
        crate::managed_services::subscription_service()?
            .add(&body.name, &body.url)
            .map_err(anyhow::Error::from)
    })
    .await?;
    let config = state.config.lock().await.clone();
    let active_id = crate::managed_services::configured_subscription(&config);
    Ok((
        StatusCode::CREATED,
        Json(dto::subscription_dto(&created, active_id)),
    )
        .into_response())
}

async fn update_subscription(
    State(state): State<Arc<ManagerState>>,
    Path(id): Path<SubscriptionId>,
    Json(body): Json<UpdateSubscriptionRequest>,
) -> Result<Json<dto::SubscriptionDto>, AppError> {
    let _permit = state
        .operation_permit
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::OperationBusy)?;
    let update = SubscriptionUpdate {
        name: body.name,
        url: body.url,
        enabled: body.enabled,
    };
    let updated = blocking(move || {
        crate::managed_services::subscription_service()?
            .update(&id.to_string(), &update)
            .map_err(anyhow::Error::from)
    })
    .await?;
    let config = state.config.lock().await.clone();
    let active_id = crate::managed_services::configured_subscription(&config);
    Ok(Json(dto::subscription_dto(&updated, active_id)))
}

async fn sync_subscription(
    State(state): State<Arc<ManagerState>>,
    Path(id): Path<SubscriptionId>,
) -> Result<Json<SubscriptionSyncSummary>, AppError> {
    let permit = state
        .operation_permit
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::OperationBusy)?;
    let subscription_id = id;
    *state.operation.lock().await = ManagerOperation::Syncing { subscription_id };
    let result = blocking(move || {
        crate::managed_services::subscription_service()?
            .sync(&subscription_id.to_string())
            .map_err(anyhow::Error::from)
    })
    .await;
    *state.operation.lock().await = ManagerOperation::Idle;
    drop(permit);
    let summary = result?;
    notify_managed_view(&state).await;
    Ok(Json(summary))
}

async fn activate_subscription(
    State(state): State<Arc<ManagerState>>,
    Path(id): Path<SubscriptionId>,
) -> Result<Json<dto::ActivateResultDto>, AppError> {
    let _permit = state
        .operation_permit
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::OperationBusy)?;
    let subscription_id = id;
    let stored = blocking(move || {
        crate::managed_services::subscription_service()?
            .find(&subscription_id.to_string())
            .map_err(anyhow::Error::from)
    })
    .await?;
    let node_count = blocking(move || {
        crate::managed_services::node_store()?
            .list()
            .map(|nodes| {
                nodes
                    .into_iter()
                    .filter(|stored| {
                        stored.node.subscription_id == subscription_id
                            && stored.node.state == ManagedNodeState::Active
                    })
                    .count()
            })
            .map_err(anyhow::Error::from)
    })
    .await?;
    if node_count == 0 {
        return Err(AppError::Conflict(
            "ACTIVATE_REQUIRES_NODES: sync the subscription first".into(),
        ));
    }
    let subscription_name = stored.source.name.clone();

    let config_path = state.config_path.clone();
    let mut updated = state.config.lock().await.clone();
    updated.proxy.mode = ProxyMode::Managed;
    updated.managed.subscription_id = subscription_id.to_string();
    updated
        .validate()
        .map_err(|error| AppError::BadRequest(error.to_string()))?;
    updated
        .save(&config_path)
        .map_err(|error| AppError::Internal(error.to_string()))?;
    *state.config.lock().await = updated.clone();

    let _ = state
        .tx
        .send(TaskResult::ManagerConfigUpdated(Ok(updated)))
        .await;
    notify_managed_view(&state).await;
    Ok(Json(dto::ActivateResultDto {
        subscription_id: subscription_id.to_string(),
        subscription_name,
    }))
}

async fn delete_subscription(
    State(state): State<Arc<ManagerState>>,
    Path(id): Path<SubscriptionId>,
) -> Result<StatusCode, AppError> {
    let _permit = state
        .operation_permit
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::OperationBusy)?;
    let config = state.config.lock().await.clone();
    if crate::managed_services::configured_subscription(&config) == Some(id) {
        return Err(AppError::Conflict(
            "ACTIVE_SUBSCRIPTION: activate another subscription first".into(),
        ));
    }
    blocking(move || {
        crate::managed_services::subscription_service()?
            .delete(&id.to_string())
            .map_err(anyhow::Error::from)
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// Nodes
// ---------------------------------------------------------------------------

#[derive(Deserialize, Default)]
pub struct NodeFilters {
    pub region: Option<CodexRegion>,
    pub state: Option<String>,
    pub subscription: Option<SubscriptionId>,
    pub q: Option<String>,
}

async fn list_nodes(
    State(state): State<Arc<ManagerState>>,
    Query(filters): Query<NodeFilters>,
) -> Result<Json<Vec<dto::NodeDto>>, AppError> {
    let config = state.config.lock().await.clone();
    let subscription = filters
        .subscription
        .or_else(|| crate::managed_services::configured_subscription(&config));
    let views = blocking(move || {
        let service = crate::managed_services::benchmark_service(&config)?;
        service
            .node_status(subscription, Utc::now())
            .map_err(anyhow::Error::from)
    })
    .await?;
    let query = filters.q.as_deref().map(str::to_lowercase);
    let nodes = views
        .iter()
        .filter(|view| {
            filters
                .region
                .is_none_or(|region| view.node.region_hint == region)
        })
        .filter(|view| match filters.state.as_deref() {
            None | Some("all") => true,
            Some("active") => view.node.state == ManagedNodeState::Active,
            Some("stale") => view.node.state == ManagedNodeState::Stale,
            Some(_) => true,
        })
        .filter(|view| {
            query
                .as_ref()
                .is_none_or(|needle| view.node.name.to_lowercase().contains(needle))
        })
        .map(dto::node_dto)
        .collect();
    Ok(Json(nodes))
}

// ---------------------------------------------------------------------------
// Benchmark
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct BenchmarkRequest {
    #[serde(default = "default_scope")]
    pub scope: String,
}

fn default_scope() -> String {
    "auto".into()
}

async fn start_benchmark(
    State(state): State<Arc<ManagerState>>,
    Json(body): Json<BenchmarkRequest>,
) -> Result<StatusCode, AppError> {
    if !matches!(body.scope.as_str(), "auto" | "all" | "JP" | "SG" | "US") {
        return Err(AppError::BadRequest(format!(
            "unsupported benchmark scope {:?}",
            body.scope
        )));
    }
    let permit = state
        .operation_permit
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::OperationBusy)?;
    *state.operation.lock().await = ManagerOperation::Benchmarking {
        started_at: Utc::now(),
    };
    let task_state = state.clone();
    tokio::spawn(async move {
        run_benchmark(&task_state, permit).await;
    });
    Ok(StatusCode::ACCEPTED)
}

async fn run_benchmark(state: &Arc<ManagerState>, permit: OwnedSemaphorePermit) {
    let config = state.config.lock().await.clone();
    let subscription_id = crate::managed_services::configured_subscription(&config);
    let cancellation = state.shutdown.child_token();
    let service = match blocking(move || crate::managed_services::benchmark_service(&config)).await
    {
        Ok(service) => service,
        Err(error) => {
            *state.operation.lock().await = ManagerOperation::Failed {
                message: error.to_string(),
            };
            drop(permit);
            return;
        }
    };
    let summary: Option<BenchmarkRunSummary> =
        match service.run(subscription_id, &cancellation).await {
            Ok(summary) => Some(summary),
            Err(error) => {
                *state.operation.lock().await = ManagerOperation::Failed {
                    message: error.to_string(),
                };
                drop(permit);
                return;
            }
        };
    let summary = summary.expect("summary present on success");
    *state.last_benchmark.lock().await = Some(summary.clone());
    *state.operation.lock().await = ManagerOperation::Idle;
    notify_managed_view(state).await;
    drop(permit);
}

async fn list_benchmark_reports(
    State(state): State<Arc<ManagerState>>,
) -> Result<Json<Vec<dto::NodeDto>>, AppError> {
    let config = state.config.lock().await.clone();
    let subscription = crate::managed_services::configured_subscription(&config);
    let views = blocking(move || {
        let service = crate::managed_services::benchmark_service(&config)?;
        service
            .node_status(subscription, Utc::now())
            .map_err(anyhow::Error::from)
    })
    .await?;
    Ok(Json(
        views
            .iter()
            .filter(|view| view.report.is_some())
            .map(dto::node_dto)
            .collect(),
    ))
}

// ---------------------------------------------------------------------------
// Manual selection
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub struct SelectionRequest {
    pub node_id: NodeId,
}

async fn set_selection(
    State(state): State<Arc<ManagerState>>,
    Json(body): Json<SelectionRequest>,
) -> Result<Json<dto::SelectionResultDto>, AppError> {
    let _permit = state
        .operation_permit
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::OperationBusy)?;
    let config = state.config.lock().await.clone();
    let Some(subscription_id) = crate::managed_services::configured_subscription(&config) else {
        return Err(AppError::Conflict(
            "NO_ACTIVE_SUBSCRIPTION: activate a subscription first".into(),
        ));
    };
    let node_id = body.node_id;
    let selection = blocking(move || {
        let service = crate::managed_services::benchmark_service(&config)?;
        service
            .healthy_selection_for(node_id, subscription_id, Utc::now())
            .map_err(anyhow::Error::from)
    })
    .await?;
    let Some(selection) = selection else {
        return Err(AppError::Conflict(
            "SELECTION_NOT_ELIGIBLE: node is stale, lacks a fresh healthy report, or does not belong to the active subscription".into(),
        ));
    };
    *state.manual_selection.lock().await = Some(selection.clone());
    let _ = state
        .tx
        .send(TaskResult::ManagerSelectionChanged(Ok(Some(
            selection.clone(),
        ))))
        .await;
    Ok(Json(dto::SelectionResultDto {
        selection: Some(selection),
    }))
}

async fn clear_selection(
    State(state): State<Arc<ManagerState>>,
) -> Result<Json<dto::SelectionResultDto>, AppError> {
    let _permit = state
        .operation_permit
        .clone()
        .try_acquire_owned()
        .map_err(|_| AppError::OperationBusy)?;
    *state.manual_selection.lock().await = None;
    let _ = state
        .tx
        .send(TaskResult::ManagerSelectionChanged(Ok(None)))
        .await;
    Ok(Json(dto::SelectionResultDto { selection: None }))
}

// ---------------------------------------------------------------------------
// Lifecycle
// ---------------------------------------------------------------------------

async fn close_manager(State(state): State<Arc<ManagerState>>) -> StatusCode {
    state.shutdown.cancel();
    StatusCode::ACCEPTED
}

// ---------------------------------------------------------------------------
// Router
// ---------------------------------------------------------------------------

pub fn build_router(state: Arc<ManagerState>) -> Router {
    let api = Router::new()
        .route("/state", get(get_state))
        .route("/operation", get(get_operation))
        .route(
            "/subscriptions",
            get(list_subscriptions).post(create_subscription),
        )
        .route("/subscriptions/inspect", post(inspect_subscription))
        .route(
            "/subscriptions/{id}",
            patch(update_subscription).delete(delete_subscription),
        )
        .route("/subscriptions/{id}/sync", post(sync_subscription))
        .route("/subscriptions/{id}/activate", post(activate_subscription))
        .route("/nodes", get(list_nodes))
        .route("/benchmark", post(start_benchmark))
        .route("/benchmark/reports", get(list_benchmark_reports))
        .route("/selection", post(set_selection).delete(clear_selection))
        .route("/manager/close", post(close_manager))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            auth::require_auth,
        ));

    Router::new()
        .route("/", get(assets::index))
        .route("/app.js", get(assets::app_js))
        .route("/style.css", get(assets::style_css))
        .nest("/api/v1", api)
        .with_state(state)
        .layer(middleware::from_fn(auth::security_headers))
}

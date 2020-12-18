use actix::Addr;
use actix_web::{delete, get, HttpRequest, HttpResponse, post, put, web};
use actix_web_actors::ws;
use diesel::prelude::*;
use log::error;

use core::db::DieselEnum;
use core::ErrorMessage;
use core::models::paginate::{CountStarOver, Paginate, PaginationRequest};
use core::responses::SuccessResponse;
use core::sanitized::SanitizedJson;
use core::schema::experiments;
use core::types::{DBPool, DefaultResponse, ModelId, Result};
use user::models::user::User;

use crate::connection::server::{ExperimentServer, RunExperimentMessage};
use crate::connection::session::Session;
use crate::models::experiment::{Experiment, ExperimentStatus};
use crate::requests::ExperimentRequest;

#[get("/ws")]
pub async fn join_server(experiment_server: web::Data<Addr<ExperimentServer>>, req: HttpRequest, stream: web::Payload) -> actix_web::Result<HttpResponse> {
    ws::start(Session::new(experiment_server.get_ref().clone()), &req, stream)
}

#[get("experiments")]
pub async fn fetch_experiments(pool: web::Data<DBPool>, user: User, pagination: web::Query<PaginationRequest>) -> DefaultResponse {
    let conn = pool.get().unwrap();

    let experiments = web::block(move || experiments::table
        .filter(experiments::user_id.eq(user.id))
        .order(experiments::created_at.desc())
        .select((experiments::all_columns, CountStarOver))
        .paginate(pagination.page)
        .per_page(pagination.per_page)
        .load_and_count_pages::<Experiment>(&conn)
    )
        .await?;

    Ok(HttpResponse::Ok().json(experiments))
}

#[get("experiment/{id}")]
pub async fn fetch_experiment(pool: web::Data<DBPool>, experiment_id: web::Path<ModelId>, user: User) -> DefaultResponse {
    let conn = pool.get().unwrap();

    let experiment = web::block(move || experiments::table
        .filter(experiments::user_id.eq(user.id))
        .find(experiment_id.into_inner())
        .first::<Experiment>(&conn)
    )
        .await?;

    Ok(HttpResponse::Ok().json(experiment))
}

#[post("experiment")]
pub async fn create_new_experiment(pool: web::Data<DBPool>, user: User, request: SanitizedJson<ExperimentRequest>) -> DefaultResponse {
    let conn = pool.get().unwrap();

    let experiment = web::block(move || diesel::insert_into(experiments::table)
        .values(
            (experiments::user_id.eq(user.id), experiments::name.eq(request.into_inner().name))
        )
        .get_result::<Experiment>(&conn)
    )
        .await?;

    Ok(HttpResponse::Ok().json(experiment))
}

/// This will return a SuccessResponse even though update may not occur if experiment's user id is not
/// equal to user.id. Update endpoints will generally behave like this.
#[put("experiment/{id}")]
pub async fn update_experiment(pool: web::Data<DBPool>, experiment_id: web::Path<ModelId>, user: User, request: SanitizedJson<ExperimentRequest>)
                               -> DefaultResponse {
    let conn = pool.get().unwrap();

    web::block(move ||
        diesel::update(
            experiments::table
                .filter(experiments::user_id.eq(user.id))
                .find(experiment_id.into_inner())
        )
            .set(experiments::name.eq(request.into_inner().name))
            .execute(&conn)
    )
        .await?;

    Ok(HttpResponse::Ok().json(SuccessResponse::default()))
}

#[put("run/{id}")]
pub async fn run_experiment(pool: web::Data<DBPool>, experiment_server: web::Data<Addr<ExperimentServer>>, experiment_id: web::Path<ModelId>, user: User)
                            -> DefaultResponse {
    let conn = pool.get().unwrap();

    let experiment = web::block(move || -> Result<Experiment> {
        // We may need to do this in transaction. However concurrent updating is not a problem for this specific endpoint
        let experiment = experiments::table
            .filter(experiments::user_id.eq(user.id))
            .find(experiment_id.into_inner())
            .first::<Experiment>(&conn)?;

        if experiment.status == ExperimentStatus::Pending || experiment.status == ExperimentStatus::Running {
            return Err(Box::new(ErrorMessage::InvalidOperationForStatus));
        } else {
            diesel::update(experiments::table.find(experiment.id))
                .set(experiments::status.eq(ExperimentStatus::Pending.value()))
                .execute(&conn)?;
        }

        Ok(experiment)
    })
        .await?;

    if let Err(e) = experiment_server.send(RunExperimentMessage { experiment_id: experiment.id })
        .await {
        error!("Error while sending experiment to ExperimentServer: {:?}", e);

        web::block(move || diesel::update(experiments::table.find(experiment.id))
            .set(experiments::status.eq(ExperimentStatus::Failed.value()))
            .execute(&pool.get().unwrap())
        )
            .await?;
    }

    Ok(HttpResponse::Ok().json(SuccessResponse::default()))
}

/// This will return a SuccessResponse even though delete may not occur if experiment's user id is not
/// equal to user.id. Delete endpoints will generally behave like this.
#[delete("experiment/{id}")]
pub async fn delete_experiment(pool: web::Data<DBPool>, experiment_id: web::Path<ModelId>, user: User) -> DefaultResponse {
    let conn = pool.get().unwrap();

    web::block(move ||
        diesel::delete(
            experiments::table
                .filter(experiments::user_id.eq(user.id))
                .find(experiment_id.into_inner())
        ).execute(&conn)
    )
        .await?;

    Ok(HttpResponse::Ok().json(SuccessResponse::default()))
}
use std::sync::Arc;

use actix_web::http::Method;
use actix_web::{web, Error as ActixWebError, HttpRequest, HttpResponse};

use juniper::http::{playground::playground_source, GraphQLRequest};

use crate::db::DbPool;
use crate::graphql_schema::{create_graphql_context, SchemaGraphQL};

pub async fn graphql(
    req: HttpRequest,
    graphql_schema: web::Data<Arc<SchemaGraphQL>>,
    data_query: Option<web::Query<GraphQLRequest>>,
    data_body: Option<web::Json<GraphQLRequest>>,
    db_pool: web::Data<DbPool>,
) -> Result<HttpResponse, ActixWebError> {
    let data = match *req.method() {
        Method::GET => data_query.unwrap().into_inner(),
        _ => data_body.unwrap().into_inner(),
    };

    let db_pool = (*db_pool).clone();
    let graphql_context = create_graphql_context(db_pool);
    let res = data.execute(&graphql_schema, &graphql_context).await;

    Ok(HttpResponse::Ok().json(res))
}

pub async fn playground() -> HttpResponse {
    let html = playground_source("/graphql", None);
    HttpResponse::Ok()
        .content_type("text/html; charset=utf-8")
        .body(html)
}

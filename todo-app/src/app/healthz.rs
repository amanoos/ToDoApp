use topcoat::{Result, context::Cx, router::route};

use crate::db::db;
use crate::features::list::List;

/// Liveness and readiness probe: `200 ok` when the app can reach its database, an error
/// otherwise. It needs no login and reveals nothing about the data; the lookup asks for a
/// list id that never exists.
#[route(GET)]
pub async fn check(cx: &Cx) -> Result<&'static str> {
    List::filter_by_id(0).first().exec(&mut db(cx)).await?;
    Ok("ok")
}

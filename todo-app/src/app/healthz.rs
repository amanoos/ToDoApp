use topcoat::{Result, context::Cx, router::route};

use crate::db::{db, id_param};

/// Liveness and readiness probe: `200 ok` when the app can reach its database, an error
/// otherwise. It needs no login and reveals nothing about the data; the lookup asks for a
/// list id that never exists, which also proves the tables are there.
#[route(GET)]
pub async fn check(cx: &Cx) -> Result<&'static str> {
    db(cx)
        .query_one(
            "SELECT TOP 1 id FROM dbo.lists WHERE id = @P1",
            &[&id_param(0)],
        )
        .await?;
    Ok("ok")
}

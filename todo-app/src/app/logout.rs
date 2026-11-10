use topcoat::{
    Result,
    context::Cx,
    router::{
        error::{SeeOther, see_other},
        href, route,
    },
};

use crate::features::user;

/// Ends the session and returns to the login page. POST only, so a link or an image
/// on another site can't log someone out.
#[route(POST)]
pub async fn logout(cx: &Cx) -> Result<SeeOther> {
    user::log_out(cx).await?;
    Ok(see_other(href!(crate::app::login::page).resolve(cx)))
}

use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        href, route,
    },
    session,
};

use crate::features::{account, user::require_user};

#[derive(Deserialize)]
struct DeleteForm {
    password: String,
    confirm: String,
}

/// Deletes the logged-in user's account and everything in it, if they give their password
/// and type their email address. On success the browser is told to forget its session
/// cookie and is sent to the login page; otherwise it goes back to the account page with an
/// error code in the URL. The password travels only in the POST body.
#[route(POST)]
pub async fn submit(cx: &Cx, Form(form): Form<DeleteForm>) -> Result<SeeOther> {
    let user = require_user(cx).await?;

    match account::delete_account(cx, user.id, &form.password, &form.confirm).await? {
        Ok(_) => {
            // The session row is already gone; this tells the browser to drop its cookie.
            session::stop(cx).await?;
            Ok(see_other(
                href!(crate::app::login::page)
                    .query([("deleted", "1")])
                    .resolve(cx),
            ))
        }
        Err(error) => Ok(see_other(
            href!(crate::app::account::page)
                .query([("error", error.code())])
                .resolve(cx),
        )),
    }
}

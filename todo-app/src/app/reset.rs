use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{SeeOther, see_other},
        href, page, query_params, route,
    },
    view::{View, attributes, view},
};

use crate::app::shell::auth_shell;
use crate::components::button::button;
use crate::components::card::{card, card_content, card_description, card_header, card_title};
use crate::components::input::input;
use crate::db::db;
use crate::features::reset::{self, ResetError};

#[query_params(error = redirect("?"))]
struct ResetQuery {
    token: Option<String>,
    error: Option<String>,
}

/// The page a reset link opens. Opening it changes nothing; the password is only changed by
/// submitting the form.
#[page]
pub async fn page(cx: &Cx) -> Result<impl View> {
    let query = query_params::<ResetQuery>(cx)?;
    let error = query.error.as_deref().and_then(ResetError::from_code);

    // Only a well-formed token that is currently valid is ever written back into the page:
    // 64 hex characters cannot carry markup, and a dead link gets no form at all.
    let token = match query.token.as_deref().and_then(reset::clean_token) {
        Some(token)
            if reset::find_valid(&db(cx), &token, reset::now())
                .await?
                .is_some() =>
        {
            Some(token)
        }
        _ => None,
    };

    Ok(view! {
        auth_shell(
            card(
                card_header(
                    card_title("Choose a new password")
                    card_description("Use 12 to 128 characters. You will be signed out everywhere.")
                )
                card_content(
                    if let Some(token) = token {
                        <form method="POST" action=(href!(submit)) class="space-y-4">
                            if let Some(error) = error {
                                <p role="alert" class="text-sm text-destructive">(error.message())</p>
                            }
                            <input type="hidden" name="token" value=(&token)>
                            <label class="block space-y-1 text-sm font-medium">
                                "New password"
                                input(attrs: attributes! {
                                    type="password"
                                    name="new"
                                    required=""
                                    autocomplete="new-password"
                                    minlength="12"
                                    maxlength="128"
                                })
                            </label>
                            <label class="block space-y-1 text-sm font-medium">
                                "Confirm new password"
                                input(attrs: attributes! {
                                    type="password"
                                    name="confirm"
                                    required=""
                                    autocomplete="new-password"
                                    minlength="12"
                                    maxlength="128"
                                })
                            </label>
                            button(attrs: attributes! { type="submit" class="w-full" }, "Set new password")
                        </form>
                    } else {
                        <p role="alert" class="text-sm text-destructive">
                            (ResetError::InvalidLink.message())
                        </p>
                        <p class="mt-4 text-sm">
                            "Ask whoever runs this site for a new link, or "
                            <a class="underline" href=(href!(crate::app::login::page))>"sign in"</a>
                            "."
                        </p>
                    }
                )
            )
        )
    })
}

#[derive(Deserialize)]
struct ResetForm {
    token: String,
    new: String,
    confirm: String,
}

/// Sets the new password if the link is valid, then sends the person to the login page. On
/// a refused password the person goes back to the form (the link is kept, so they can try
/// again); on a bad link they get the "not valid" page. Nobody is logged in by this.
#[route(POST)]
pub async fn submit(cx: &Cx, Form(form): Form<ResetForm>) -> Result<SeeOther> {
    match reset::redeem_for_request(cx, &form.token, &form.new, &form.confirm, reset::now()).await?
    {
        Ok(()) => Ok(see_other(
            href!(crate::app::login::page)
                .query([("reset", "1")])
                .resolve(cx),
        )),
        Err(error) => {
            // The token goes back into the address only if it is well formed.
            let mut pairs = Vec::new();
            let token = reset::clean_token(&form.token);
            if let Some(token) = &token {
                pairs.push(("token", token.as_str()));
            }
            pairs.push(("error", error.code()));
            Ok(see_other(href!(page).query(pairs).resolve(cx)))
        }
    }
}

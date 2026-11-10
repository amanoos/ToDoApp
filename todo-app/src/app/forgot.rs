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
use crate::features::forgot::{self, Admission};

#[query_params(error = redirect("?"))]
struct ForgotQuery {
    /// Set to `1` after a request, whatever the address was.
    sent: Option<String>,
    /// Set to `slow` when this network has asked too many times.
    error: Option<String>,
}

/// Asks for a password reset link to be emailed. Available only when the site is set up to
/// send email (`SMTP_URL`); otherwise it explains that the operator has to supply the link.
#[page]
pub async fn page(cx: &Cx) -> Result<impl View> {
    let query = query_params::<ForgotQuery>(cx)?;
    let enabled = forgot::enabled(cx);
    let sent = query.sent.as_deref() == Some("1");
    let too_many = query.error.as_deref() == Some("slow");

    Ok(view! {
        auth_shell(
            card(
                card_header(
                    card_title("Forgot your password?")
                    card_description("We will email you a link to choose a new one.")
                )
                card_content(
                    if !enabled {
                        <p role="status" class="text-sm">
                            "This site cannot send email. Ask whoever runs it for a password reset link."
                        </p>
                    } else if sent {
                        // The same words whether or not the address has an account.
                        <p role="status" class="text-sm font-medium">
                            "If that address belongs to an account, a link to choose a new password is on its way. It works once, for 60 minutes."
                        </p>
                    } else {
                        <form method="POST" action=(href!(submit)) class="space-y-4">
                            if too_many {
                                <p role="alert" class="text-sm text-destructive">
                                    "Too many requests from your network. Try again in a few minutes."
                                </p>
                            }
                            <label class="block space-y-1 text-sm font-medium">
                                "Email"
                                input(attrs: attributes! {
                                    type="email"
                                    name="email"
                                    required=""
                                    autocomplete="email"
                                    maxlength="320"
                                })
                            </label>
                            button(attrs: attributes! { type="submit" class="w-full" }, "Email me a link")
                        </form>
                    }
                    <p class="mt-4 text-sm">
                        <a class="underline" href=(href!(crate::app::login::page))>"Back to sign in"</a>
                    </p>
                )
            )
        )
    })
}

#[derive(Deserialize)]
struct ForgotForm {
    email: String,
}

/// Starts the email in the background and answers at once, the same way for every address.
#[route(POST)]
pub async fn submit(cx: &Cx, Form(form): Form<ForgotForm>) -> Result<SeeOther> {
    let query = match forgot::request(cx, &form.email) {
        Admission::TooManyFromNetwork => [("error", "slow")],
        Admission::Send | Admission::Ignore => [("sent", "1")],
    };
    Ok(see_other(href!(page).query(query).resolve(cx)))
}

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
use crate::features::user::{self, AuthError};

#[query_params(error = redirect("?"))]
struct LoginQuery {
    error: Option<String>,
    /// Set to `1` after an account was deleted, to say so once.
    deleted: Option<String>,
    /// Set to `1` after a password was reset, to say so once.
    reset: Option<String>,
}

#[page]
pub async fn page(cx: &Cx) -> Result<impl View> {
    let query = query_params::<LoginQuery>(cx)?;
    let error = query.error.as_deref().and_then(AuthError::from_code);
    let deleted = query.deleted.as_deref() == Some("1");
    let was_reset = query.reset.as_deref() == Some("1");
    // The link to the sign-up page is only offered while sign-ups are possible.
    let open = user::registration_open(cx).await?;
    // Likewise the "forgot your password?" link, which needs the site to be able to send email.
    let reset_by_email = crate::features::forgot::enabled(cx);

    Ok(view! {
        auth_shell(
            card(
                card_header(
                    card_title("Sign in")
                    card_description("Welcome back.")
                )
                card_content(
                    if deleted {
                        <p role="status" class="mb-4 text-sm font-medium">
                            "Your account was deleted."
                        </p>
                    }
                    if was_reset {
                        <p role="status" class="mb-4 text-sm font-medium">
                            "Your password was changed and every device was signed out. Sign in with the new password."
                        </p>
                    }
                    <form method="POST" action=(href!(submit)) class="space-y-4">
                        if let Some(error) = error {
                            <p role="alert" class="text-sm text-destructive">(error.message())</p>
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
                        <label class="block space-y-1 text-sm font-medium">
                            "Password"
                            input(attrs: attributes! {
                                type="password"
                                name="password"
                                required=""
                                autocomplete="current-password"
                                maxlength="128"
                            })
                        </label>
                        button(attrs: attributes! { type="submit" class="w-full" }, "Sign in")
                    </form>
                    if reset_by_email {
                        <p class="mt-4 text-sm">
                            <a class="underline" href=(href!(crate::app::forgot::page))>"Forgot your password?"</a>
                        </p>
                    }
                    if open {
                        <p class="mt-4 text-sm">
                            "New here? "
                            <a class="underline" href=(href!(crate::app::register::page))>"Create an account"</a>
                        </p>
                    }
                )
            )
        )
    })
}

#[derive(Deserialize)]
struct LoginForm {
    email: String,
    password: String,
}

/// Checks the credentials, then redirects to the todo list, or back here with an error.
#[route(POST)]
pub async fn submit(cx: &Cx, Form(form): Form<LoginForm>) -> Result<SeeOther> {
    match user::log_in(cx, &form.email, &form.password).await? {
        Ok(_) => Ok(see_other(href!(crate::app::todos::page).resolve(cx))),
        Err(error) => Ok(see_other(
            href!(page).query([("error", error.code())]).resolve(cx),
        )),
    }
}

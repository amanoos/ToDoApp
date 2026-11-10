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

use crate::components::button::button;
use crate::components::card::{card, card_content, card_description, card_header, card_title};
use crate::components::input::input;
use crate::features::user::{self, AuthError};

#[query_params(error = redirect("?"))]
struct RegisterQuery {
    error: Option<String>,
}

#[page]
pub async fn page(cx: &Cx) -> Result<impl View> {
    let query = query_params::<RegisterQuery>(cx)?;
    let error = query.error.as_deref().and_then(AuthError::from_code);
    let open = user::registration_open(cx).await?;

    Ok(view! {
        <main class="mx-auto max-w-sm px-4 py-16">
            card(
                card_header(
                    card_title("Create an account")
                    card_description("Use a password of at least 12 characters.")
                )
                card_content(
                    if open {
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
                                autocomplete="new-password"
                                minlength="12"
                                maxlength="128"
                            })
                        </label>
                        button(attrs: attributes! { type="submit" class="w-full" }, "Create account")
                    </form>
                    } else {
                        <p role="status" class="text-sm">
                            "Sign-ups are closed. Ask the owner of this site for an account."
                        </p>
                    }
                    <p class="mt-4 text-sm">
                        "Already have an account? "
                        <a class="underline" href=(href!(crate::app::login::page))>"Sign in"</a>
                    </p>
                )
            )
        </main>
    })
}

#[derive(Deserialize)]
struct RegisterForm {
    email: String,
    password: String,
}

/// Creates the account and logs it in, or redirects back here with an error.
#[route(POST)]
pub async fn submit(cx: &Cx, Form(form): Form<RegisterForm>) -> Result<SeeOther> {
    match user::register(cx, &form.email, &form.password).await? {
        Ok(_) => Ok(see_other(href!(crate::app::todos::page).resolve(cx))),
        Err(error) => Ok(see_other(
            href!(page).query([("error", error.code())]).resolve(cx),
        )),
    }
}

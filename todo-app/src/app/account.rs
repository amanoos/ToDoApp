pub(crate) mod delete;

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

use crate::app::shell::{Nav, app_shell};
use crate::components::button::{ButtonVariant, button};
use crate::components::card::{card, card_content, card_description, card_header, card_title};
use crate::components::icon::glyph;
use crate::components::input::input;
use crate::features::account::DeleteError;
use crate::features::user::{self, PasswordError, require_user};
use topcoat::icon::iconify::iconify_icon;

#[query_params(error = redirect("?"))]
struct AccountQuery {
    error: Option<String>,
    ok: Option<String>,
}

#[page]
pub async fn page(cx: &Cx) -> Result<impl View> {
    let user = require_user(cx).await?;
    let query = query_params::<AccountQuery>(cx)?;
    // The two forms on this page report their errors through the same `error` parameter;
    // the codes never overlap, so each form picks out its own.
    let error = query.error.as_deref().and_then(PasswordError::from_code);
    let delete_error = query.error.as_deref().and_then(DeleteError::from_code);
    let changed = query.ok.as_deref() == Some("1");

    Ok(view! {
        app_shell(
            email: user.email.clone(),
            active: Nav::Account,
            <div class="rise mb-6 flex items-center gap-4">
                <span class="brand-gradient grid size-12 shrink-0 place-items-center rounded-2xl text-primary-foreground shadow-sm">
                    glyph(data: iconify_icon!("lucide:user-round"), attrs: attributes! { class="size-6" })
                </span>
                <div>
                    <h1 class="text-2xl font-bold tracking-tight">"Your account"</h1>
                    <p class="text-sm text-muted-foreground">"Signed in as " (&user.email)</p>
                </div>
            </div>
            <div class="grid items-start gap-6 lg:grid-cols-2">
            card(
                card_header(
                    card_title("Change password")
                    card_description("Signed in as " (&user.email))
                )
                card_content(
                    if changed {
                        <p role="status" class="mb-4 text-sm font-medium">
                            "Your password was changed. Every other device was signed out."
                        </p>
                    }
                    <form method="POST" action=(href!(submit)) class="space-y-4">
                        if let Some(error) = error {
                            <p role="alert" class="text-sm text-destructive">(error.message())</p>
                        }
                        <label class="block space-y-1 text-sm font-medium">
                            "Current password"
                            input(attrs: attributes! {
                                type="password"
                                name="current"
                                required=""
                                autocomplete="current-password"
                                maxlength="128"
                            })
                        </label>
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
                        <p class="text-xs text-muted-foreground">
                            "Use 12 to 128 characters. Changing your password signs out every other device."
                        </p>
                        button(attrs: attributes! { type="submit" class="w-full" }, "Change password")
                    </form>
                )
            )
            <div>
                card(
                    card_header(
                        card_title("Delete account")
                        card_description("This permanently deletes your account and everything in it.")
                    )
                    card_content(
                        <p class="text-sm">
                            "All of your todos and lists will be deleted, and you will be signed out on every device. This cannot be undone."
                        </p>
                        <form method="POST" action=(href!(crate::app::account::delete::submit)) class="mt-4 space-y-4">
                            if let Some(error) = delete_error {
                                <p role="alert" class="text-sm text-destructive">(error.message())</p>
                            }
                            <label class="block space-y-1 text-sm font-medium">
                                "Your password"
                                input(attrs: attributes! {
                                    type="password"
                                    name="password"
                                    required=""
                                    autocomplete="current-password"
                                    maxlength="128"
                                })
                            </label>
                            <label class="block space-y-1 text-sm font-medium">
                                "To confirm, type your email address: " (&user.email)
                                input(attrs: attributes! {
                                    type="text"
                                    name="confirm"
                                    required=""
                                    autocomplete="off"
                                    maxlength="320"
                                })
                            </label>
                            button(
                                variant: ButtonVariant::Destructive,
                                attrs: attributes! { type="submit" class="w-full" },
                                "Delete my account permanently"
                            )
                        </form>
                    )
                )
            </div>
            </div>
        )
    })
}

#[derive(Deserialize)]
struct PasswordForm {
    current: String,
    new: String,
    confirm: String,
}

/// Changes the password, then returns to the account page with a notice or an error code.
/// The passwords travel only in the POST body, never in a URL.
#[route(POST)]
pub async fn submit(cx: &Cx, Form(form): Form<PasswordForm>) -> Result<SeeOther> {
    let user = require_user(cx).await?;
    let back =
        |error: PasswordError| see_other(href!(page).query([("error", error.code())]).resolve(cx));

    if form.new != form.confirm {
        return Ok(back(PasswordError::Mismatch));
    }
    match user::change_password(cx, user.id, &form.current, &form.new).await? {
        Ok(()) => Ok(see_other(href!(page).query([("ok", "1")]).resolve(cx))),
        Err(error) => Ok(back(error)),
    }
}

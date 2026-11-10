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

use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::input::input;
use crate::features::list::{List, ListError, MAX_NAME_LEN};
use crate::features::todo::Todo;
use crate::features::user::require_user;

#[query_params(error = redirect("?"))]
struct ListsQuery {
    error: Option<String>,
}

/// One list and how many todos are in it.
struct Row {
    id: String,
    name: String,
    count: String,
}

#[page]
pub async fn page(cx: &Cx) -> Result<impl View> {
    let user = require_user(cx).await?;
    let query = query_params::<ListsQuery>(cx)?;
    let error = query.error.as_deref().and_then(ListError::from_code);

    let todos = Todo::list(cx, user.id).await?;
    let rows: Vec<Row> = List::list(cx, user.id)
        .await?
        .into_iter()
        .map(|list| {
            let count = todos.iter().filter(|t| t.list_id == Some(list.id)).count();
            Row {
                id: list.id.to_string(),
                name: list.name,
                count: if count == 1 {
                    "1 todo".to_owned()
                } else {
                    format!("{count} todos")
                },
            }
        })
        .collect();
    let max_len = MAX_NAME_LEN.to_string();

    Ok(view! {
        <main class="mx-auto max-w-xl px-4 py-16">
            <div class="flex items-center justify-between gap-4">
                <h1 class="text-3xl font-semibold tracking-tight">"Lists"</h1>
                <a class="text-sm underline" href=(href!(crate::app::todos::page))>"Back to todos"</a>
            </div>
            if let Some(error) = error {
                <p role="alert" class="mt-4 text-sm text-destructive">(error.message())</p>
            }
            <form method="POST" action=(href!(submit)) class="mt-4 flex gap-2">
                <input type="hidden" name="action" value="create">
                <div class="flex-1">
                    input(attrs: attributes! {
                        type="text"
                        name="name"
                        placeholder="New list name"
                        aria-label="New list name"
                        required=""
                        maxlength=(&max_len)
                    })
                </div>
                button(attrs: attributes! { type="submit" }, "Add list")
            </form>
            <p class="mt-2 text-sm text-muted-foreground">
                "Deleting a list keeps its todos: they move to the Inbox."
            </p>
            if rows.is_empty() {
                <p class="mt-6">"No lists yet."</p>
            } else {
                <ul class="mt-6 space-y-2">
                    for row in rows {
                        <li class="rounded-lg border border-border p-3">
                            <div class="flex flex-wrap items-center gap-2">
                                <form method="POST" action=(href!(submit)) class="flex min-w-48 flex-1 gap-2">
                                    <input type="hidden" name="action" value="rename">
                                    <input type="hidden" name="id" value=(&row.id)>
                                    <div class="flex-1">
                                        input(attrs: attributes! {
                                            type="text"
                                            name="name"
                                            aria-label="List name"
                                            required=""
                                            maxlength=(&max_len)
                                            value=(&row.name)
                                        })
                                    </div>
                                    button(
                                        variant: ButtonVariant::Outline,
                                        size: ButtonSize::Sm,
                                        attrs: attributes! { type="submit" },
                                        "Save"
                                    )
                                </form>
                                <span class="text-xs text-muted-foreground">(&row.count)</span>
                                <form method="POST" action=(href!(submit))>
                                    <input type="hidden" name="action" value="delete">
                                    <input type="hidden" name="id" value=(&row.id)>
                                    button(
                                        variant: ButtonVariant::Ghost,
                                        size: ButtonSize::Sm,
                                        attrs: attributes! { type="submit" },
                                        "Delete"
                                    )
                                </form>
                            </div>
                        </li>
                    }
                </ul>
            }
        </main>
    })
}

#[derive(Deserialize)]
struct ListForm {
    /// `create`, `rename` or `delete`.
    action: String,
    #[serde(default)]
    id: u64,
    #[serde(default)]
    name: String,
}

/// Creates, renames or deletes a list, then returns to the lists page, with an error
/// code in the URL if the change was refused.
#[route(POST)]
pub async fn submit(cx: &Cx, Form(form): Form<ListForm>) -> Result<SeeOther> {
    let user = require_user(cx).await?;

    let refused: Option<ListError> = match form.action.as_str() {
        "create" => List::add(cx, user.id, &form.name).await?.err(),
        "rename" => List::rename(cx, user.id, form.id, &form.name).await?.err(),
        "delete" => (!List::remove(cx, user.id, form.id).await?).then_some(ListError::NotFound),
        _ => Some(ListError::NotFound),
    };

    Ok(see_other(match refused {
        None => href!(page).resolve(cx),
        Some(error) => href!(page).query([("error", error.code())]).resolve(cx),
    }))
}

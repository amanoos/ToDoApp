use serde::Deserialize;
use topcoat::{
    Result,
    context::Cx,
    router::{
        content::Form,
        error::{RouterErrorExt, SeeOther, see_other},
        href, page, query_params, route,
    },
    view::{View, attributes, view},
};

use crate::components::button::{ButtonSize, ButtonVariant, button, button_variants};
use crate::components::card::{card, card_content, card_header, card_title};
use crate::components::input::input;
use crate::features::list::List;
use crate::features::todo::{FieldError, MAX_NOTES_LEN, Priority, Todo, TodoInput};
use crate::features::user::require_user;

const PRIORITIES: [Priority; 3] = [Priority::Low, Priority::Medium, Priority::High];

#[query_params(error = redirect("?"))]
struct EditQuery {
    id: Option<u64>,
    error: Option<String>,
}

#[page]
pub async fn page(cx: &Cx) -> Result<impl View> {
    let user = require_user(cx).await?;
    let query = query_params::<EditQuery>(cx)?;
    let error = query.error.as_deref().and_then(FieldError::from_code);

    // A missing id, or a todo that is not yours, goes back to the list.
    let todo = match query.id {
        Some(id) => Todo::find_owned(cx, user.id, id).await?,
        None => None,
    }
    .ok_or_redirect(href!(crate::app::todos::page).resolve(cx))?;

    let id_text = todo.id.to_string();
    let notes = todo.notes.clone().unwrap_or_default();
    let due = todo.due_on.clone().unwrap_or_default();
    let current = todo.priority();
    let lists = List::list(cx, user.id).await?;
    let current_list = todo.list_id;

    Ok(view! {
        <main class="mx-auto max-w-xl px-4 py-16">
            card(
                card_header(card_title("Edit todo"))
                card_content(
                    <form method="POST" action=(href!(submit)) class="space-y-4">
                        if let Some(error) = error {
                            <p role="alert" class="text-sm text-destructive">(error.message())</p>
                        }
                        <input type="hidden" name="id" value=(&id_text)>
                        <label class="block space-y-1 text-sm font-medium">
                            "Title"
                            input(attrs: attributes! {
                                type="text"
                                name="title"
                                required=""
                                maxlength="200"
                                value=(&todo.title)
                            })
                        </label>
                        <label class="block space-y-1 text-sm font-medium">
                            "Notes"
                            <textarea
                                name="notes"
                                rows="4"
                                maxlength=(MAX_NOTES_LEN.to_string())
                                class="w-full rounded-lg border border-border bg-transparent px-3 py-2 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring"
                            >(&notes)</textarea>
                        </label>
                        <div class="flex flex-wrap gap-4">
                            <label class="block space-y-1 text-sm font-medium">
                                "Priority"
                                <select
                                    name="priority"
                                    class="block h-9 rounded-lg border border-border bg-transparent px-3 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring"
                                >
                                    for option in PRIORITIES {
                                        if option == current {
                                            <option value=(option.as_str()) selected="">(option.label())</option>
                                        } else {
                                            <option value=(option.as_str())>(option.label())</option>
                                        }
                                    }
                                </select>
                            </label>
                            <label class="block space-y-1 text-sm font-medium">
                                "List"
                                <select
                                    name="list"
                                    class="block h-9 rounded-lg border border-border bg-transparent px-3 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring"
                                >
                                    if current_list.is_none() {
                                        <option value="inbox" selected="">"Inbox"</option>
                                    } else {
                                        <option value="inbox">"Inbox"</option>
                                    }
                                    for list in lists.iter() {
                                        if current_list == Some(list.id) {
                                            <option value=(list.id.to_string()) selected="">(&list.name)</option>
                                        } else {
                                            <option value=(list.id.to_string())>(&list.name)</option>
                                        }
                                    }
                                </select>
                            </label>
                            <label class="block space-y-1 text-sm font-medium">
                                "Due date"
                                input(attrs: attributes! {
                                    type="date"
                                    name="due"
                                    value=(&due)
                                })
                            </label>
                        </div>
                        <div class="flex gap-2">
                            button(attrs: attributes! { type="submit" }, "Save")
                            <a
                                class=(button_variants(ButtonVariant::Outline, ButtonSize::Md))
                                href=(href!(crate::app::todos::page))
                            >
                                "Cancel"
                            </a>
                        </div>
                    </form>
                )
            )
        </main>
    })
}

#[derive(Deserialize)]
struct EditForm {
    id: u64,
    title: String,
    #[serde(default)]
    notes: String,
    #[serde(default)]
    priority: String,
    #[serde(default)]
    due: String,
    #[serde(default)]
    list: String,
}

/// Saves the changes and returns to the list, or returns here with an error.
#[route(POST)]
pub async fn submit(cx: &Cx, Form(form): Form<EditForm>) -> Result<SeeOther> {
    let user = require_user(cx).await?;
    let list = href!(crate::app::todos::page).resolve(cx);

    let back_with = |error: FieldError| {
        let id = form.id.to_string();
        see_other(
            href!(page)
                .query([("id", id.as_str()), ("error", error.code())])
                .resolve(cx),
        )
    };

    let input = match TodoInput::parse_with_list(
        &form.title,
        &form.notes,
        &form.priority,
        &form.due,
        &form.list,
    ) {
        Ok(input) => input,
        Err(error) => return Ok(back_with(error)),
    };

    // A list that doesn't exist or isn't yours is an error the person should see.
    if let Some(list_id) = input.list_id
        && List::find_owned(cx, user.id, list_id).await?.is_none()
    {
        return Ok(back_with(FieldError::List));
    }

    // A todo that doesn't exist, or isn't yours, changes nothing; the result is the same.
    Todo::save(cx, user.id, form.id, &input).await?;
    Ok(see_other(list))
}

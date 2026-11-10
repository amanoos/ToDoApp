pub(crate) mod edit;

use std::collections::HashMap;

use topcoat::{
    Result,
    context::Cx,
    router::{href, page, query_params},
    runtime::{Event, Signal, procedure, shard, signal},
    view::{View, attributes, view},
};

use crate::components::button::{ButtonSize, ButtonVariant, button, button_variants};
use crate::components::input::input;
use crate::features::list::List;
use crate::features::todo::{
    Counts, Date, DueStatus, Filter, ListScope, Priority, Sort, Todo, TodoInput, clean_query,
    select, sort_todos,
};
use crate::features::user::{refresh_session, require_user};

/// The view state carried in the URL, so a view can be shared or reloaded. Values that
/// are missing or unrecognised fall back to their defaults.
#[query_params(error = redirect("?"))]
struct TodosQuery {
    filter: Option<String>,
    list: Option<String>,
    sort: Option<String>,
    q: Option<String>,
}

/// Classes for a `<select>`, matching the input component.
const SELECT: &str = "h-9 rounded-lg border border-border bg-transparent px-3 text-sm outline-none focus-visible:ring-2 focus-visible:ring-ring";

/// What the todo list is showing: the filter, list, sort order and search text. Every
/// link on the page is built from one of these, so changing one setting keeps the rest.
#[derive(Clone)]
struct Params {
    filter: Filter,
    scope: ListScope,
    sort: Sort,
    query: String,
}

impl Params {
    /// Builds the state from raw text that came from the browser.
    fn parse(filter: &str, list: &str, sort: &str, query: &str) -> Self {
        Self {
            filter: Filter::parse(filter).unwrap_or_default(),
            scope: ListScope::parse(list).unwrap_or_default(),
            sort: Sort::parse(sort).unwrap_or_default(),
            query: clean_query(query),
        }
    }

    /// A link to the todo page in this state. Settings at their defaults are left out,
    /// which keeps URLs short.
    fn href(&self, cx: &Cx) -> String {
        let mut pairs: Vec<(&str, String)> = Vec::new();
        if self.filter != Filter::default() {
            pairs.push(("filter", self.filter.code().to_owned()));
        }
        if self.scope != ListScope::default() {
            pairs.push(("list", self.scope.code()));
        }
        if self.sort != Sort::default() {
            pairs.push(("sort", self.sort.code().to_owned()));
        }
        if !self.query.is_empty() {
            pairs.push(("q", self.query.clone()));
        }
        if pairs.is_empty() {
            return href!(page).resolve(cx).to_string();
        }
        let pairs: Vec<(&str, &str)> = pairs.iter().map(|(k, v)| (*k, v.as_str())).collect();
        href!(page).query(pairs).resolve(cx).to_string()
    }

    fn with_filter(&self, filter: Filter) -> Self {
        Self {
            filter,
            ..self.clone()
        }
    }

    fn with_scope(&self, scope: ListScope) -> Self {
        Self {
            scope,
            ..self.clone()
        }
    }

    fn with_sort(&self, sort: Sort) -> Self {
        Self {
            sort,
            ..self.clone()
        }
    }

    /// The same list and sort, with no filter or search.
    fn cleared(&self) -> Self {
        Self {
            filter: Filter::default(),
            query: String::new(),
            ..self.clone()
        }
    }
}

/// A scope that names a list the user doesn't have counts as All lists.
fn valid_scope(scope: ListScope, lists: &[List]) -> ListScope {
    match scope {
        ListScope::List(id) if !lists.iter().any(|list| list.id == id) => ListScope::All,
        other => other,
    }
}

#[page]
pub async fn page(cx: &Cx) -> Result<impl View> {
    // Logged-out visitors are redirected to the login page.
    let user = require_user(cx).await?;
    // Keeps an active session alive. It sets a cookie, so it runs before the page renders.
    refresh_session(cx).await?;

    let lists = List::list(cx, user.id).await?;
    let url = query_params::<TodosQuery>(cx)?;
    let params = Params::parse(
        url.filter.as_deref().unwrap_or_default(),
        url.list.as_deref().unwrap_or_default(),
        url.sort.as_deref().unwrap_or_default(),
        url.q.as_deref().unwrap_or_default(),
    );
    let scope = valid_scope(params.scope, &lists);
    let filter_code = params.filter.code().to_owned();
    let list_code = scope.code();
    let sort_code = params.sort.code().to_owned();

    // The values of the add form and the search box, kept in the browser. New todos go
    // into the list being viewed, if any.
    let search = signal(cx, || params.query.clone());
    let title = signal(cx, String::new);
    let priority = signal(cx, || "medium".to_owned());
    let due = signal(cx, String::new);
    let list_choice = signal(cx, || match scope {
        ListScope::List(id) => id.to_string(),
        _ => "inbox".to_owned(),
    });
    // Increased after every change, so the list renders again with the new todos.
    let revision = signal(cx, || 0u32);

    Ok(view! {
        <main class="mx-auto max-w-2xl px-4 py-16">
            <div class="flex items-center justify-between gap-4">
                <h1 class="text-3xl font-semibold tracking-tight">"Todos"</h1>
                <form method="POST" action=(href!(crate::app::logout::logout)) class="flex items-center gap-3">
                    <span class="text-sm text-muted-foreground">(&user.email)</span>
                    <a class="text-sm underline" href=(href!(crate::app::account::page))>"Account"</a>
                    button(
                        variant: ButtonVariant::Outline,
                        size: ButtonSize::Sm,
                        attrs: attributes! { type="submit" },
                        "Log out"
                    )
                </form>
            </div>
            <form
                class="mt-4 flex flex-wrap gap-2"
                @submit=$(async |e: Event| {
                    e.prevent_default();
                    add(title.get(), priority.get(), due.get(), list_choice.get()).await;
                    title.set("".to_owned());
                    due.set("".to_owned());
                    revision.increment();
                })
            >
                <div class="min-w-48 flex-1">
                    input(
                        attrs: attributes! {
                            type="text"
                            placeholder="What needs doing?"
                            aria-label="Title"
                            required=""
                            maxlength="200"
                            :value=$(title.get())
                            @input=$(|e: Event| title.set(e.target.value))
                        }
                    )
                </div>
                <select
                    class=(SELECT)
                    aria-label="List"
                    :value=$(list_choice.get())
                    @input=$(|e: Event| list_choice.set(e.target.value))
                >
                    <option value="inbox">"Inbox"</option>
                    for list in lists.iter() {
                        <option value=(list.id.to_string())>(&list.name)</option>
                    }
                </select>
                <select
                    class=(SELECT)
                    aria-label="Priority"
                    :value=$(priority.get())
                    @input=$(|e: Event| priority.set(e.target.value))
                >
                    <option value="low">"Low"</option>
                    <option value="medium">"Medium"</option>
                    <option value="high">"High"</option>
                </select>
                <div class="w-40">
                    input(
                        attrs: attributes! {
                            type="date"
                            aria-label="Due date"
                            :value=$(due.get())
                            @input=$(|e: Event| due.set(e.target.value))
                        }
                    )
                </div>
                button(attrs: attributes! { type="submit" }, "Add")
            </form>
            // Pressing Enter submits this as a plain GET, which puts the search in the
            // URL; typing alone updates the list below without leaving the page.
            <form method="GET" action=(href!(page)) role="search" class="mt-4">
                <input type="hidden" name="filter" value=(&filter_code)>
                <input type="hidden" name="list" value=(&list_code)>
                <input type="hidden" name="sort" value=(&sort_code)>
                input(
                    attrs: attributes! {
                        type="search"
                        name="q"
                        placeholder="Search todos"
                        aria-label="Search todos"
                        maxlength="100"
                        autocomplete="off"
                        :value=$(search.get())
                        @input=$(|e: Event| search.set(e.target.value))
                    }
                )
            </form>
            todo_list(
                revision: revision,
                query: $(search.get()),
                filter: filter_code.clone(),
                list: list_code.clone(),
                sort: sort_code.clone()
            )
        </main>
    })
}

/// The text for a due date, and whether it is urgent (overdue and not yet done).
fn due_text(due: Date, today: Date, done: bool) -> (String, bool) {
    match due.status(today) {
        DueStatus::Overdue if !done => (format!("Overdue: {}", due.to_iso()), true),
        DueStatus::Today if !done => ("Due today".to_owned(), true),
        _ => (format!("Due {}", due.to_iso()), false),
    }
}

/// One tab or link: where it goes, what it says, and whether it is the current one.
struct Tab {
    label: String,
    count: Option<usize>,
    active: bool,
    href: String,
}

/// The list tabs, status tabs, sort links and the todo list, rendered on the server again
/// whenever `revision` or the search text changes. Every argument except `revision` comes
/// from the browser, so each is cleaned and validated here.
#[shard]
pub async fn todo_list(
    cx: &Cx,
    revision: Signal<u32>,
    query: String,
    filter: String,
    list: String,
    sort: String,
) -> Result<impl View> {
    // Reading the revision on the server makes the list depend on it.
    let _ = revision.get();
    let user = require_user(cx).await?;

    let lists = List::list(cx, user.id).await?;
    let mut params = Params::parse(&filter, &list, &sort, &query);
    params.scope = valid_scope(params.scope, &lists);
    let scope = params.scope;

    let all = Todo::list(cx, user.id).await?;
    let list_names: HashMap<u64, String> = lists.iter().map(|l| (l.id, l.name.clone())).collect();

    // List tabs count every todo in the list, whatever its status.
    let mut list_tabs = vec![
        Tab {
            label: "All lists".to_owned(),
            count: Some(all.len()),
            active: scope == ListScope::All,
            href: params.with_scope(ListScope::All).href(cx),
        },
        Tab {
            label: "Inbox".to_owned(),
            count: Some(all.iter().filter(|t| t.list_id.is_none()).count()),
            active: scope == ListScope::Inbox,
            href: params.with_scope(ListScope::Inbox).href(cx),
        },
    ];
    for list in &lists {
        list_tabs.push(Tab {
            label: list.name.clone(),
            count: Some(all.iter().filter(|t| t.list_id == Some(list.id)).count()),
            active: scope == ListScope::List(list.id),
            href: params.with_scope(ListScope::List(list.id)).href(cx),
        });
    }

    // Everything below is about the todos in the chosen list only.
    let in_scope: Vec<Todo> = all
        .into_iter()
        .filter(|t| scope.contains(t.list_id))
        .collect();
    let counts = Counts::of(&in_scope);
    let scope_total = in_scope.len();
    let mut todos = select(in_scope, params.filter, &params.query);
    sort_todos(&mut todos, params.sort);
    let today = Date::today();
    let show_list_names = scope == ListScope::All;

    let status_tabs: Vec<Tab> = Filter::ALL
        .into_iter()
        .map(|tab| Tab {
            label: tab.label().to_owned(),
            count: Some(counts.get(tab)),
            active: tab == params.filter,
            href: params.with_filter(tab).href(cx),
        })
        .collect();
    let sort_links: Vec<Tab> = Sort::ALL
        .into_iter()
        .map(|option| Tab {
            label: option.label().to_owned(),
            count: None,
            active: option == params.sort,
            href: params.with_sort(option).href(cx),
        })
        .collect();
    let clear_href = params.cleared().href(cx);
    let lists_href = href!(crate::app::lists::page).resolve(cx).to_string();

    Ok(view! {
        <nav class="mt-4 flex flex-wrap items-center gap-1" aria-label="Lists">
            for tab in list_tabs {
                if tab.active {
                    <a
                        class=(button_variants(ButtonVariant::Secondary, ButtonSize::Sm))
                        href=(&tab.href)
                        aria-current="page"
                    >
                        (&tab.label) " (" (tab.count.unwrap_or(0)) ")"
                    </a>
                } else {
                    <a
                        class=(button_variants(ButtonVariant::Ghost, ButtonSize::Sm))
                        href=(&tab.href)
                    >
                        (&tab.label) " (" (tab.count.unwrap_or(0)) ")"
                    </a>
                }
            }
            <a class="ml-auto text-sm underline" href=(&lists_href)>"Manage lists"</a>
        </nav>
        <nav class="mt-2 flex flex-wrap gap-1" aria-label="Filter todos">
            for tab in status_tabs {
                if tab.active {
                    <a
                        class=(button_variants(ButtonVariant::Outline, ButtonSize::Sm))
                        href=(&tab.href)
                        aria-current="page"
                    >
                        (&tab.label) " (" (tab.count.unwrap_or(0)) ")"
                    </a>
                } else {
                    <a
                        class=(button_variants(ButtonVariant::Ghost, ButtonSize::Sm))
                        href=(&tab.href)
                    >
                        (&tab.label) " (" (tab.count.unwrap_or(0)) ")"
                    </a>
                }
            }
        </nav>
        <nav class="mt-2 flex flex-wrap items-center gap-3 text-sm" aria-label="Sort todos">
            <span class="text-muted-foreground">"Sort by"</span>
            for link in sort_links {
                if link.active {
                    <a class="font-semibold underline" href=(&link.href) aria-current="page">(&link.label)</a>
                } else {
                    <a class="text-muted-foreground hover:underline" href=(&link.href)>(&link.label)</a>
                }
            }
        </nav>
        if todos.is_empty() {
            if scope_total == 0 {
                if scope == ListScope::All {
                    <p class="mt-4">"Nothing to do."</p>
                } else {
                    <p class="mt-4">"Nothing in this list yet."</p>
                }
            } else {
                <p class="mt-4">
                    "No todos match. "
                    <a class="underline" href=(&clear_href)>"Clear filter and search"</a>
                </p>
            }
        } else {
            <ul class="mt-4 space-y-2">
                for todo in todos {
                    // Browser code can only capture plain values, such as the id.
                    let id = todo.id;
                    let id_text = id.to_string();
                    let priority_label = todo.priority().label();
                    let high = todo.priority() == Priority::High;
                    let due_label = todo.due().map(|due| due_text(due, today, todo.done));
                    let notes = todo.notes.clone();
                    let list_name = todo
                        .list_id
                        .and_then(|id| list_names.get(&id).cloned())
                        .filter(|_| show_list_names);
                    <li class="rounded-lg border border-border p-3">
                        <div class="flex flex-wrap items-center gap-2">
                            button(
                                variant: ButtonVariant::Outline,
                                size: ButtonSize::Sm,
                                attrs: attributes! {
                                    @click=$(async |_e: Event| {
                                        toggle(id).await;
                                        revision.increment();
                                    })
                                },
                                if todo.done {
                                    "Undo"
                                } else {
                                    "Done"
                                }
                            )
                            <span class="min-w-32 flex-1">
                                if todo.done {
                                    <s>(&todo.title)</s>
                                } else {
                                    <span>(&todo.title)</span>
                                }
                            </span>
                            if let Some(name) = list_name {
                                <span class="rounded-full border border-border px-2 py-0.5 text-xs text-muted-foreground">
                                    "List: " (&name)
                                </span>
                            }
                            if high {
                                <span class="rounded-full border border-foreground px-2 py-0.5 text-xs font-semibold">
                                    "Priority: " (priority_label)
                                </span>
                            } else {
                                <span class="rounded-full border border-border px-2 py-0.5 text-xs text-muted-foreground">
                                    "Priority: " (priority_label)
                                </span>
                            }
                            if let Some((label, urgent)) = due_label {
                                if urgent {
                                    <span class="rounded-full border border-destructive px-2 py-0.5 text-xs font-semibold text-destructive">(&label)</span>
                                } else {
                                    <span class="rounded-full border border-border px-2 py-0.5 text-xs text-muted-foreground">(&label)</span>
                                }
                            }
                            <a
                                class=(button_variants(ButtonVariant::Ghost, ButtonSize::Sm))
                                href=(href!(crate::app::todos::edit::page).query([("id", id_text.as_str())]))
                            >
                                "Edit"
                            </a>
                            button(
                                variant: ButtonVariant::Ghost,
                                size: ButtonSize::Sm,
                                attrs: attributes! {
                                    @click=$(async |_e: Event| {
                                        remove(id).await;
                                        revision.increment();
                                    })
                                },
                                "Delete"
                            )
                        </div>
                        if let Some(notes) = notes {
                            <p class="mt-2 whitespace-pre-wrap text-sm text-muted-foreground">(&notes)</p>
                        }
                    </li>
                }
            </ul>
        }
    })
}

// Procedures run on the server when the browser calls them. Their arguments come from
// the browser, so validate them like any other request data.

/// Adds a todo for the logged-in user. Input that fails validation, or that names a list
/// the user doesn't own, is ignored; the browser form already limits what it sends, so
/// this only happens to tampered requests.
#[procedure]
pub async fn add(
    cx: &Cx,
    title: String,
    priority: String,
    due: String,
    list: String,
) -> Result<()> {
    let user = require_user(cx).await?;
    if let Ok(input) = TodoInput::parse_with_list(&title, "", &priority, &due, &list) {
        Todo::add(cx, user.id, &input).await?;
    }
    Ok(())
}

/// Marks one of the user's todos as done, or as not done if it already is.
#[procedure]
pub async fn toggle(cx: &Cx, id: u64) -> Result<()> {
    let user = require_user(cx).await?;
    Todo::toggle(cx, user.id, id).await
}

/// Removes one of the user's todos.
#[procedure]
pub async fn remove(cx: &Cx, id: u64) -> Result<()> {
    let user = require_user(cx).await?;
    Todo::remove(cx, user.id, id).await
}

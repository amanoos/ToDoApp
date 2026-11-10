pub(crate) mod edit;

use std::collections::HashMap;

use topcoat::{
    Result,
    context::Cx,
    icon::iconify::iconify_icon,
    router::{href, page, query_params},
    runtime::{Event, Signal, procedure, shard, signal},
    view::{View, attributes, view},
};

use crate::app::shell::{Nav, app_shell};
use crate::components::button::{ButtonSize, ButtonVariant, button, button_variants};
use crate::components::icon::glyph;
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
const SELECT: &str = "h-9 rounded-lg border border-border bg-card px-3 text-sm outline-none transition-colors focus-visible:ring-2 focus-visible:ring-ring";

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
    let display_name = user.email.split('@').next().unwrap_or("there").to_owned();
    let today_text = Date::today().to_iso();

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
    // Increased after every change, so everything that shows todos renders again.
    let revision = signal(cx, || 0u32);

    Ok(view! {
        app_shell(
            email: user.email.clone(),
            active: Nav::Todos,
            wide: true,
            <div class="grid gap-6 lg:grid-cols-[16.5rem_minmax(0,1fr)] xl:grid-cols-[16.5rem_minmax(0,1fr)_21rem]">
                <aside class="order-2 lg:sticky lg:top-24 lg:order-none lg:self-start" aria-label="Lists and sorting">
                    lists_rail(
                        revision: revision.clone(),
                        query: $(search.get()),
                        filter: filter_code.clone(),
                        list: list_code.clone(),
                        sort: sort_code.clone()
                    )
                </aside>
                <div class="order-1 min-w-0 space-y-5 lg:order-none">
                    <section class="banner rise relative isolate overflow-hidden rounded-3xl p-6 text-white shadow-lift sm:p-8">
                        <div class="relative z-10 max-w-lg">
                            <p class="flex items-center gap-2.5 text-sm font-medium text-white/85">
                                <span class="pulse-dot relative inline-flex size-2.5 rounded-full bg-accent text-accent" aria-hidden="true"></span>
                                (&today_text)
                            </p>
                            <h1 class="mt-2 text-3xl font-bold tracking-tight sm:text-4xl">"Welcome back, " (&display_name)</h1>
                            <p class="mt-2 max-w-md text-white/85">
                                "Add what is on your mind, tick things off, and watch the ring fill."
                            </p>
                        </div>
                        <div class="art-empty float pointer-events-none absolute -right-4 bottom-0 hidden h-full w-72 sm:block" aria-hidden="true"></div>
                    </section>
                    <form
                        class="glass rise rounded-2xl p-4 sm:p-5"
                        aria-label="Add a todo"
                        @submit=$(async |e: Event| {
                            e.prevent_default();
                            add(title.get(), priority.get(), due.get(), list_choice.get()).await;
                            title.set("".to_owned());
                            due.set("".to_owned());
                            revision.increment();
                        })
                    >
                        <div class="flex items-center gap-2 text-sm font-semibold">
                            <span class="grid size-7 place-items-center rounded-lg bg-primary/10 text-primary">
                                glyph(data: iconify_icon!("lucide:plus"), attrs: attributes! { class="size-4" })
                            </span>
                            "New todo"
                        </div>
                        <div class="mt-3">
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
                        <div class="mt-3 flex flex-wrap items-center gap-2">
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
                                <option value="low">"Low priority"</option>
                                <option value="medium">"Medium priority"</option>
                                <option value="high">"High priority"</option>
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
                            <div class="ml-auto">
                                button(
                                    attrs: attributes! { type="submit" },
                                    glyph(data: iconify_icon!("lucide:plus"), attrs: attributes! { class="size-4" })
                                    "Add todo"
                                )
                            </div>
                        </div>
                    </form>
                    // Pressing Enter submits this as a plain GET, which puts the search in the
                    // URL; typing alone updates the list below without leaving the page.
                    <form method="GET" action=(href!(page)) role="search" class="rise relative">
                        <input type="hidden" name="filter" value=(&filter_code)>
                        <input type="hidden" name="list" value=(&list_code)>
                        <input type="hidden" name="sort" value=(&sort_code)>
                        <span class="pointer-events-none absolute left-3.5 top-1/2 -translate-y-1/2 text-muted-foreground">
                            glyph(data: iconify_icon!("lucide:search"), attrs: attributes! { class="size-4" })
                        </span>
                        input(
                            attrs: attributes! {
                                type="search"
                                name="q"
                                class="pl-10"
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
                        revision: revision.clone(),
                        query: $(search.get()),
                        filter: filter_code.clone(),
                        list: list_code.clone(),
                        sort: sort_code.clone()
                    )
                </div>
                <aside class="order-3 lg:order-none lg:col-span-2 xl:col-span-1 xl:sticky xl:top-24 xl:self-start" aria-label="Progress">
                    side_panel(revision: revision.clone())
                </aside>
            </div>
        )
    })
}

/// The text for a due date, and how urgent it is.
fn due_text(due: Date, today: Date, done: bool) -> (String, DueStatus) {
    match due.status(today) {
        DueStatus::Overdue if !done => (format!("Overdue: {}", due.to_iso()), DueStatus::Overdue),
        DueStatus::Today if !done => ("Due today".to_owned(), DueStatus::Today),
        _ => (format!("Due {}", due.to_iso()), DueStatus::Later),
    }
}

/// One tab or link: where it goes, what it says, and whether it is the current one.
struct Tab {
    label: String,
    count: Option<usize>,
    active: bool,
    href: String,
    /// 0 for All lists, 1 for the Inbox, 2 for a list of the user's own.
    kind: u8,
    /// Classes for the colored dot in front of one of the user's own lists.
    dot: &'static str,
}

/// Dots in front of the user's lists, so each is recognisable at a glance.
const DOTS: [&str; 6] = [
    "size-2.5 shrink-0 rounded-full bg-accent",
    "size-2.5 shrink-0 rounded-full bg-info",
    "size-2.5 shrink-0 rounded-full bg-success",
    "size-2.5 shrink-0 rounded-full bg-warning",
    "size-2.5 shrink-0 rounded-full bg-primary-2",
    "size-2.5 shrink-0 rounded-full bg-destructive",
];

/// Classes for one segment of the status switch.
fn segment_class(active: bool) -> &'static str {
    if active {
        "inline-flex items-center gap-2 rounded-lg bg-primary px-3.5 py-1.5 text-sm font-semibold text-primary-foreground shadow-xs transition-colors"
    } else {
        "inline-flex items-center gap-2 rounded-lg px-3.5 py-1.5 text-sm font-medium text-muted-foreground transition-colors hover:bg-foreground/5 hover:text-foreground"
    }
}

/// Classes for the count beside a status segment.
fn segment_count_class(active: bool) -> &'static str {
    if active {
        "rounded-full bg-white/25 px-2 text-xs font-semibold"
    } else {
        "rounded-full bg-foreground/5 px-2 text-xs font-semibold"
    }
}

/// Classes for a row in the side navigation.
fn rail_class(active: bool) -> &'static str {
    if active {
        "flex items-center gap-2.5 rounded-xl bg-primary/10 px-3 py-2 text-sm font-semibold text-primary transition-colors"
    } else {
        "flex items-center gap-2.5 rounded-xl px-3 py-2 text-sm font-medium text-foreground/80 transition-colors hover:bg-foreground/5 hover:text-foreground"
    }
}

/// Classes for a todo's card: the left edge takes the priority's color.
fn card_class(priority: Priority, done: bool) -> &'static str {
    match (priority, done) {
        (Priority::High, false) => {
            "lift glass flex items-start gap-3 rounded-2xl border-l-4 border-l-destructive p-4"
        }
        (Priority::Medium, false) => {
            "lift glass flex items-start gap-3 rounded-2xl border-l-4 border-l-warning p-4"
        }
        (Priority::Low, false) => {
            "lift glass flex items-start gap-3 rounded-2xl border-l-4 border-l-info p-4"
        }
        (Priority::High, true) => {
            "lift glass flex items-start gap-3 rounded-2xl border-l-4 border-l-destructive p-4 opacity-70"
        }
        (Priority::Medium, true) => {
            "lift glass flex items-start gap-3 rounded-2xl border-l-4 border-l-warning p-4 opacity-70"
        }
        (Priority::Low, true) => {
            "lift glass flex items-start gap-3 rounded-2xl border-l-4 border-l-info p-4 opacity-70"
        }
    }
}

/// Classes for the priority chip.
fn priority_chip(priority: Priority) -> &'static str {
    match priority {
        Priority::High => {
            "inline-flex items-center gap-1 rounded-full bg-destructive/10 px-2.5 py-0.5 text-xs font-semibold text-destructive"
        }
        Priority::Medium => {
            "inline-flex items-center gap-1 rounded-full bg-warning/25 px-2.5 py-0.5 text-xs font-semibold text-foreground"
        }
        Priority::Low => {
            "inline-flex items-center gap-1 rounded-full bg-info/15 px-2.5 py-0.5 text-xs font-semibold text-primary-2"
        }
    }
}

/// Classes for the due-date chip.
fn due_chip(status: DueStatus) -> &'static str {
    match status {
        DueStatus::Overdue => {
            "inline-flex items-center gap-1 rounded-full bg-destructive/10 px-2.5 py-0.5 text-xs font-semibold text-destructive"
        }
        DueStatus::Today => {
            "inline-flex items-center gap-1 rounded-full bg-warning/25 px-2.5 py-0.5 text-xs font-semibold text-foreground"
        }
        DueStatus::Later => {
            "inline-flex items-center gap-1 rounded-full bg-foreground/5 px-2.5 py-0.5 text-xs font-medium text-muted-foreground"
        }
    }
}

/// The round tick box in front of a todo.
fn check_class(done: bool) -> &'static str {
    if done {
        "pop mt-0.5 grid size-7 shrink-0 place-items-center rounded-full bg-success text-white shadow-xs outline-none transition-transform hover:scale-110 focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background"
    } else {
        "group mt-0.5 grid size-7 shrink-0 place-items-center rounded-full border-2 border-muted-foreground/40 text-transparent outline-none transition-all hover:scale-110 hover:border-success hover:text-success focus-visible:ring-2 focus-visible:ring-ring focus-visible:ring-offset-2 focus-visible:ring-offset-background"
    }
}

/// What share of `total` is `done`, as an exact percentage and rounded down to the nearest
/// five (the steps the progress ring has styles for).
fn percent(done: usize, total: usize) -> (usize, usize) {
    if total == 0 {
        return (0, 0);
    }
    let exact = done * 100 / total;
    (exact, exact / 5 * 5)
}

/// Loads the lists and settles the view state for a shard, from its raw browser arguments.
async fn view_state(
    cx: &Cx,
    user_id: u64,
    filter: &str,
    list: &str,
    sort: &str,
    query: &str,
) -> Result<(Vec<List>, Params)> {
    let lists = List::list(cx, user_id).await?;
    let mut params = Params::parse(filter, list, sort, query);
    params.scope = valid_scope(params.scope, &lists);
    Ok((lists, params))
}

/// The side navigation: the user's lists with counts, and the sort orders. It renders again
/// whenever `revision` or the search text changes. Every argument except `revision` comes
/// from the browser, so each is cleaned and validated here.
#[shard]
pub async fn lists_rail(
    cx: &Cx,
    revision: Signal<u32>,
    query: String,
    filter: String,
    list: String,
    sort: String,
) -> Result<impl View> {
    let _ = revision.get();
    let user = require_user(cx).await?;
    let (lists, params) = view_state(cx, user.id, &filter, &list, &sort, &query).await?;
    let scope = params.scope;
    let all = Todo::list(cx, user.id).await?;

    // List tabs count every todo in the list, whatever its status.
    let mut tabs = vec![
        Tab {
            label: "All todos".to_owned(),
            count: Some(all.len()),
            active: scope == ListScope::All,
            href: params.with_scope(ListScope::All).href(cx),
            kind: 0,
            dot: "",
        },
        Tab {
            label: "Inbox".to_owned(),
            count: Some(all.iter().filter(|t| t.list_id.is_none()).count()),
            active: scope == ListScope::Inbox,
            href: params.with_scope(ListScope::Inbox).href(cx),
            kind: 1,
            dot: "",
        },
    ];
    for (index, list) in lists.iter().enumerate() {
        tabs.push(Tab {
            label: list.name.clone(),
            count: Some(all.iter().filter(|t| t.list_id == Some(list.id)).count()),
            active: scope == ListScope::List(list.id),
            href: params.with_scope(ListScope::List(list.id)).href(cx),
            kind: 2,
            dot: DOTS[index % DOTS.len()],
        });
    }
    let sort_links: Vec<Tab> = Sort::ALL
        .into_iter()
        .map(|option| Tab {
            label: option.label().to_owned(),
            count: None,
            active: option == params.sort,
            href: params.with_sort(option).href(cx),
            kind: 0,
            dot: "",
        })
        .collect();
    let lists_href = href!(crate::app::lists::page).resolve(cx).to_string();

    Ok(view! {
        <div class="glass rise rounded-2xl p-3">
            <h2 class="px-3 pb-2 pt-1 text-xs font-semibold uppercase tracking-wider text-muted-foreground">"Lists"</h2>
            <nav aria-label="Lists" class="stagger grid gap-1">
                for tab in tabs {
                    <a
                        class=(rail_class(tab.active))
                        href=(&tab.href)
                        aria-current=(if tab.active { "page" } else { "false" })
                    >
                        if tab.kind == 0 {
                            glyph(data: iconify_icon!("lucide:layers"), attrs: attributes! { class="size-4 shrink-0" })
                        } else if tab.kind == 1 {
                            glyph(data: iconify_icon!("lucide:inbox"), attrs: attributes! { class="size-4 shrink-0" })
                        } else {
                            <span class=(tab.dot) aria-hidden="true"></span>
                        }
                        <span class="min-w-0 flex-1 truncate">(&tab.label)</span>
                        " "
                        <span class="rounded-full bg-foreground/5 px-2 text-xs font-semibold">(tab.count.unwrap_or(0))</span>
                    </a>
                }
            </nav>
            <a
                class="mt-2 flex items-center gap-2 rounded-xl px-3 py-2 text-sm text-muted-foreground transition-colors hover:bg-foreground/5 hover:text-foreground"
                href=(&lists_href)
            >
                glyph(data: iconify_icon!("lucide:settings-2"), attrs: attributes! { class="size-4" })
                "Manage lists"
            </a>
        </div>
        <div class="glass rise mt-4 rounded-2xl p-3">
            <h2 class="flex items-center gap-2 px-3 pb-2 pt-1 text-xs font-semibold uppercase tracking-wider text-muted-foreground">
                glyph(data: iconify_icon!("lucide:arrow-up-down"), attrs: attributes! { class="size-3.5" })
                "Sort by"
            </h2>
            <nav aria-label="Sort todos" class="grid gap-1">
                for link in sort_links {
                    <a
                        class=(rail_class(link.active))
                        href=(&link.href)
                        aria-current=(if link.active { "page" } else { "false" })
                    >
                        (&link.label)
                    </a>
                }
            </nav>
        </div>
    })
}

/// The progress panel: a ring, four counters and the next few things to do. It renders
/// again whenever `revision` changes.
#[shard]
pub async fn side_panel(cx: &Cx, revision: Signal<u32>) -> Result<impl View> {
    let _ = revision.get();
    let user = require_user(cx).await?;
    let mut todos = Todo::list(cx, user.id).await?;
    let today = Date::today();

    let total = todos.len();
    let done = todos.iter().filter(|t| t.done).count();
    let active = total - done;
    let status_of = |t: &Todo| t.due().map(|d| d.status(today));
    let overdue = todos
        .iter()
        .filter(|t| !t.done && status_of(t) == Some(DueStatus::Overdue))
        .count();
    let due_today = todos
        .iter()
        .filter(|t| !t.done && status_of(t) == Some(DueStatus::Today))
        .count();
    let (exact, step) = percent(done, total);
    let ring = format!("ring pct-{step} mx-auto grid size-44 place-items-center");
    let message = match (total, exact) {
        (0, _) => "Add a todo to get started.",
        (_, 100) => "Everything is done. Enjoy the quiet.",
        (_, 50..) => "Past halfway. Keep going.",
        _ => "Pick one small thing to start.",
    };

    // The next few things to do: soonest due first.
    sort_todos(&mut todos, Sort::Due);
    let next: Vec<(String, Option<(String, DueStatus)>)> = todos
        .iter()
        .filter(|t| !t.done)
        .take(4)
        .map(|t| {
            (
                t.title.clone(),
                t.due().map(|due| due_text(due, today, t.done)),
            )
        })
        .collect();

    Ok(view! {
        <div class="glass rise rounded-2xl p-5 text-center">
            <h2 class="text-xs font-semibold uppercase tracking-wider text-muted-foreground">"Progress"</h2>
            <div class=(&ring) role="img" aria-label=(format!("{exact} percent done"))>
                <div class="relative z-10">
                    <p class="text-4xl font-bold tracking-tight">(exact) <span class="text-2xl text-muted-foreground">"%"</span></p>
                    <p class="text-xs text-muted-foreground">(done) " of " (total) " done"</p>
                </div>
            </div>
            <p class="mt-3 text-sm text-muted-foreground">(message)</p>
        </div>
        <div class="stagger mt-4 grid grid-cols-2 gap-3">
            <div class="glass lift rounded-2xl p-4">
                <span class="grid size-9 place-items-center rounded-xl bg-info/15 text-primary-2">
                    glyph(data: iconify_icon!("lucide:circle-dot"), attrs: attributes! { class="size-5" })
                </span>
                <p class="mt-3 text-2xl font-bold">(active)</p>
                <p class="text-xs text-muted-foreground">"Active"</p>
            </div>
            <div class="glass lift rounded-2xl p-4">
                <span class="grid size-9 place-items-center rounded-xl bg-success/15 text-success">
                    glyph(data: iconify_icon!("lucide:circle-check-big"), attrs: attributes! { class="size-5" })
                </span>
                <p class="mt-3 text-2xl font-bold">(done)</p>
                <p class="text-xs text-muted-foreground">"Completed"</p>
            </div>
            <div class="glass lift rounded-2xl p-4">
                <span class="grid size-9 place-items-center rounded-xl bg-warning/25 text-foreground">
                    glyph(data: iconify_icon!("lucide:alarm-clock"), attrs: attributes! { class="size-5" })
                </span>
                <p class="mt-3 text-2xl font-bold">(due_today)</p>
                <p class="text-xs text-muted-foreground">"Due today"</p>
            </div>
            <div class="glass lift rounded-2xl p-4">
                <span class="grid size-9 place-items-center rounded-xl bg-destructive/10 text-destructive">
                    glyph(data: iconify_icon!("lucide:flame"), attrs: attributes! { class="size-5" })
                </span>
                <p class="mt-3 text-2xl font-bold">(overdue)</p>
                <p class="text-xs text-muted-foreground">"Overdue"</p>
            </div>
        </div>
        <div class="glass rise mt-4 rounded-2xl p-4">
            <h2 class="flex items-center gap-2 text-xs font-semibold uppercase tracking-wider text-muted-foreground">
                glyph(data: iconify_icon!("lucide:sparkles"), attrs: attributes! { class="size-3.5" })
                "Up next"
            </h2>
            if next.is_empty() {
                <p class="mt-3 text-sm text-muted-foreground">"Nothing waiting. Nice."</p>
            } else {
                <ul class="mt-3 grid gap-2">
                    for item in next {
                        <li class="rounded-xl bg-foreground/5 px-3 py-2">
                            <p class="truncate text-sm font-medium">(&item.0)</p>
                            if let Some((label, status)) = item.1 {
                                <span class=(due_chip(status))>(&label)</span>
                            }
                        </li>
                    }
                </ul>
            }
        </div>
    })
}

/// The status switch and the todo cards, rendered on the server again whenever `revision` or
/// the search text changes. Every argument except `revision` comes from the browser, so each
/// is cleaned and validated here.
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
    let (lists, params) = view_state(cx, user.id, &filter, &list, &sort, &query).await?;
    let scope = params.scope;

    let all = Todo::list(cx, user.id).await?;
    let list_names: HashMap<u64, String> = lists.iter().map(|l| (l.id, l.name.clone())).collect();

    // Everything below is about the todos in the chosen list only.
    let in_scope: Vec<Todo> = all
        .into_iter()
        .filter(|t| scope.contains(t.list_id))
        .collect();
    let counts = Counts::of(&in_scope);
    let scope_total = in_scope.len();
    let mut todos = select(in_scope, params.filter, &params.query);
    sort_todos(&mut todos, params.sort);
    let shown = todos.len();
    let today = Date::today();
    let show_list_names = scope == ListScope::All;
    let heading = match scope {
        ListScope::All => "All todos".to_owned(),
        ListScope::Inbox => "Inbox".to_owned(),
        ListScope::List(id) => list_names.get(&id).cloned().unwrap_or_default(),
    };

    let status_tabs: Vec<Tab> = Filter::ALL
        .into_iter()
        .map(|tab| Tab {
            label: tab.label().to_owned(),
            count: Some(counts.get(tab)),
            active: tab == params.filter,
            href: params.with_filter(tab).href(cx),
            kind: 0,
            dot: "",
        })
        .collect();
    let clear_href = params.cleared().href(cx);

    Ok(view! {
        <div class="flex flex-wrap items-center justify-between gap-3">
            <h2 class="text-xl font-bold tracking-tight">
                (&heading)
                " "
                <span class="text-base font-medium text-muted-foreground">"(" (shown) ")"</span>
            </h2>
            <nav class="glass inline-flex gap-1 rounded-xl p-1" aria-label="Filter todos">
                for tab in status_tabs {
                    <a
                        class=(segment_class(tab.active))
                        href=(&tab.href)
                        aria-current=(if tab.active { "page" } else { "false" })
                    >
                        (&tab.label)
                        " "
                        <span class=(segment_count_class(tab.active))>(tab.count.unwrap_or(0))</span>
                    </a>
                }
            </nav>
        </div>
        if todos.is_empty() {
            <div class="glass rise flex flex-col items-center rounded-3xl px-6 py-10 text-center">
                <div class="art-empty float aspect-[280/200] w-64 max-w-full" role="img" aria-label="An illustration of a clipboard with a checklist"></div>
                if scope_total == 0 {
                    <h3 class="mt-2 text-xl font-bold">"Nothing to do"</h3>
                    <p class="mt-1 max-w-sm text-muted-foreground">
                        "A clear list. Add a todo above to get started."
                    </p>
                } else {
                    <h3 class="mt-2 text-xl font-bold">"No todos match"</h3>
                    <p class="mt-1 max-w-sm text-muted-foreground">
                        "Nothing fits this filter or search."
                    </p>
                    <a
                        class=(button_variants(ButtonVariant::Outline, ButtonSize::Md))
                        href=(&clear_href)
                    >
                        "Clear filter and search"
                    </a>
                }
            </div>
        } else {
            <ul class="stagger grid gap-3">
                for todo in todos {
                    // Browser code can only capture plain values, such as the id.
                    let id = todo.id;
                    let id_text = id.to_string();
                    let priority = todo.priority();
                    let priority_label = priority.label();
                    let due_label = todo.due().map(|due| due_text(due, today, todo.done));
                    let notes = todo.notes.clone();
                    let list_name = todo
                        .list_id
                        .and_then(|id| list_names.get(&id).cloned())
                        .filter(|_| show_list_names);
                    let toggle_label = if todo.done { "Mark as not done" } else { "Mark as done" };
                    let pressed = if todo.done { "true" } else { "false" };
                    let delete_label = format!("Delete {}", todo.title);
                    let edit_label = format!("Edit {}", todo.title);
                    <li class=(card_class(priority, todo.done))>
                        <button
                            type="button"
                            class=(check_class(todo.done))
                            aria-label=(toggle_label)
                            aria-pressed=(pressed)
                            @click=$(async |_e: Event| {
                                toggle(id).await;
                                revision.increment();
                            })
                        >
                            glyph(data: iconify_icon!("lucide:check"), attrs: attributes! { class="size-4" })
                        </button>
                        <div class="min-w-0 flex-1">
                            <p class="break-words text-base">
                                if todo.done {
                                    <s class="text-muted-foreground">(&todo.title)</s>
                                } else {
                                    <span class="font-medium">(&todo.title)</span>
                                }
                            </p>
                            if let Some(notes) = notes {
                                <p class="mt-1 whitespace-pre-wrap text-sm text-muted-foreground">(&notes)</p>
                            }
                            <div class="mt-2 flex flex-wrap items-center gap-1.5">
                                if let Some(name) = list_name {
                                    <span class="inline-flex items-center gap-1 rounded-full bg-primary/10 px-2.5 py-0.5 text-xs font-semibold text-primary">
                                        glyph(data: iconify_icon!("lucide:folder"), attrs: attributes! { class="size-3" })
                                        (&name)
                                    </span>
                                }
                                <span class=(priority_chip(priority))>
                                    glyph(data: iconify_icon!("lucide:flag"), attrs: attributes! { class="size-3" })
                                    (priority_label)
                                </span>
                                if let Some((label, status)) = due_label {
                                    <span class=(due_chip(status))>
                                        glyph(data: iconify_icon!("lucide:calendar"), attrs: attributes! { class="size-3" })
                                        (&label)
                                    </span>
                                }
                            </div>
                        </div>
                        <div class="flex shrink-0 items-center gap-1">
                            <a
                                class="grid size-9 place-items-center rounded-lg text-muted-foreground outline-none transition-colors hover:bg-primary/10 hover:text-primary focus-visible:ring-2 focus-visible:ring-ring"
                                href=(href!(crate::app::todos::edit::page).query([("id", id_text.as_str())]))
                                aria-label=(&edit_label)
                            >
                                glyph(data: iconify_icon!("lucide:pencil"), attrs: attributes! { class="size-4" })
                            </a>
                            <button
                                type="button"
                                class="grid size-9 place-items-center rounded-lg text-muted-foreground outline-none transition-colors hover:bg-destructive/10 hover:text-destructive focus-visible:ring-2 focus-visible:ring-ring"
                                aria-label=(&delete_label)
                                @click=$(async |_e: Event| {
                                    remove(id).await;
                                    revision.increment();
                                })
                            >
                                glyph(data: iconify_icon!("lucide:trash-2"), attrs: attributes! { class="size-4" })
                            </button>
                        </div>
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

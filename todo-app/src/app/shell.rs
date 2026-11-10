//! The frame around every page: the signed-in top bar and the split-screen sign-in layout.
//!
//! Both are components that take the page's content as children. They hold no routes, so
//! this module contributes nothing to the router.

use topcoat::{
    Result,
    context::Cx,
    icon::iconify::iconify_icon,
    router::href,
    view::{Child, View, attributes, component, view},
};

use crate::components::button::{ButtonSize, ButtonVariant, button};
use crate::components::icon::glyph;

/// Which entry of the top bar is highlighted.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Nav {
    Todos,
    Lists,
    Account,
    /// A page that is not in the bar, such as editing a single todo.
    #[default]
    None,
}

/// Classes for a top-bar link, depending on whether it is the current page.
fn nav_class(active: bool) -> &'static str {
    if active {
        "inline-flex h-9 items-center gap-2 rounded-lg bg-primary/10 px-3 text-sm font-semibold text-primary transition-colors"
    } else {
        "inline-flex h-9 items-center gap-2 rounded-lg px-3 text-sm font-medium text-muted-foreground transition-colors hover:bg-foreground/5 hover:text-foreground"
    }
}

/// The first letter of an email address, in capitals, for the avatar.
fn initial(email: &str) -> String {
    email
        .chars()
        .next()
        .map(|c| c.to_uppercase().collect())
        .unwrap_or_else(|| "?".to_owned())
}

/// The page frame for signed-in pages: a sticky top bar over a wide content area.
///
/// `wide` lets the content use the full width of large screens (the todo dashboard);
/// otherwise it stays in a comfortable column.
#[component]
pub async fn app_shell(
    cx: &Cx,
    email: String,
    #[default] active: Nav,
    #[default] wide: bool,
    #[default] child: Child<'_>,
) -> Result<impl View> {
    let avatar = initial(&email);
    let content = if wide {
        "mx-auto w-full max-w-[1680px] flex-1 px-4 py-6 sm:px-6 lg:px-8 lg:py-8"
    } else {
        "mx-auto w-full max-w-5xl flex-1 px-4 py-8 sm:px-6 lg:py-12"
    };

    Ok(view! {
        <div class="flex min-h-dvh flex-col">
            <header class="glass-strong sticky top-0 z-30 border-x-0 border-t-0">
                <div class="mx-auto flex w-full max-w-[1680px] flex-wrap items-center gap-x-3 gap-y-1 px-4 py-2 sm:px-6 lg:px-8">
                    <a
                        class="flex items-center gap-2.5 rounded-lg outline-none focus-visible:ring-2 focus-visible:ring-ring"
                        href=(href!(crate::app::todos::page))
                    >
                        <span class="brand-gradient grid size-9 place-items-center rounded-xl text-primary-foreground shadow-sm">
                            glyph(
                                data: iconify_icon!("lucide:check-check"),
                                attrs: attributes! { class="size-5" }
                            )
                        </span>
                        <span class="brand-text text-lg font-bold tracking-tight">"Topcoat To-Do"</span>
                    </a>
                    <nav aria-label="Main" class="order-last flex w-full items-center gap-1 pb-1 sm:order-none sm:ml-4 sm:w-auto sm:pb-0">
                        if active == Nav::Todos {
                            <a class=(nav_class(true)) href=(href!(crate::app::todos::page)) aria-current="page">
                                glyph(data: iconify_icon!("lucide:list-checks"), attrs: attributes! { class="size-4" })
                                "Todos"
                            </a>
                        } else {
                            <a class=(nav_class(false)) href=(href!(crate::app::todos::page))>
                                glyph(data: iconify_icon!("lucide:list-checks"), attrs: attributes! { class="size-4" })
                                "Todos"
                            </a>
                        }
                        if active == Nav::Lists {
                            <a class=(nav_class(true)) href=(href!(crate::app::lists::page)) aria-current="page">
                                glyph(data: iconify_icon!("lucide:folders"), attrs: attributes! { class="size-4" })
                                "Lists"
                            </a>
                        } else {
                            <a class=(nav_class(false)) href=(href!(crate::app::lists::page))>
                                glyph(data: iconify_icon!("lucide:folders"), attrs: attributes! { class="size-4" })
                                "Lists"
                            </a>
                        }
                        if active == Nav::Account {
                            <a class=(nav_class(true)) href=(href!(crate::app::account::page)) aria-current="page">
                                glyph(data: iconify_icon!("lucide:user-round"), attrs: attributes! { class="size-4" })
                                "Account"
                            </a>
                        } else {
                            <a class=(nav_class(false)) href=(href!(crate::app::account::page))>
                                glyph(data: iconify_icon!("lucide:user-round"), attrs: attributes! { class="size-4" })
                                "Account"
                            </a>
                        }
                    </nav>
                    <form method="POST" action=(href!(crate::app::logout::logout)) class="ml-auto flex items-center gap-3">
                        <span class="hidden items-center gap-2 md:flex">
                            <span
                                class="grid size-8 place-items-center rounded-full bg-accent text-sm font-bold text-white"
                                aria-hidden="true"
                            >
                                (&avatar)
                            </span>
                            <span class="max-w-56 truncate text-sm text-muted-foreground">(&email)</span>
                        </span>
                        button(
                            variant: ButtonVariant::Outline,
                            size: ButtonSize::Sm,
                            attrs: attributes! { type="submit" },
                            glyph(data: iconify_icon!("lucide:log-out"), attrs: attributes! { class="size-4" })
                            "Log out"
                        )
                    </form>
                </div>
            </header>
            <main class=(content)>(child)</main>
            <footer class="px-4 pb-6 text-center text-xs text-muted-foreground">
                "Built with Rust and Topcoat"
            </footer>
        </div>
    })
}

/// The frame for the signed-out pages: an illustrated panel beside the form.
///
/// On narrow screens the panel is hidden and the form fills the window.
#[component]
pub async fn auth_shell(cx: &Cx, #[default] child: Child<'_>) -> Result<impl View> {
    Ok(view! {
        <div class="grid min-h-dvh lg:grid-cols-[minmax(0,1.05fr)_minmax(0,1fr)]">
            <aside class="auth-panel relative hidden flex-col justify-between overflow-hidden p-10 text-white lg:flex xl:p-14">
                <div class="relative flex items-center gap-3">
                    <span class="grid size-11 place-items-center rounded-2xl bg-white/20 shadow-sm ring-1 ring-white/30">
                        glyph(
                            data: iconify_icon!("lucide:check-check"),
                            attrs: attributes! { class="size-6" }
                        )
                    </span>
                    <span class="text-xl font-bold tracking-tight">"Topcoat To-Do"</span>
                </div>
                <div class="relative mx-auto my-6 flex w-full max-w-xl flex-1 items-center justify-center">
                    <div
                        class="art-hero float aspect-[520/560] max-h-[52vh] w-full"
                        role="img"
                        aria-label="Illustration of a checklist with finished tasks"
                    ></div>
                </div>
                <div class="relative">
                    <h2 class="max-w-md text-3xl font-bold tracking-tight xl:text-4xl">"Get the small things done."</h2>
                    <ul class="mt-5 grid gap-3 text-sm text-white/90">
                        <li class="flex items-center gap-3">
                            <span class="grid size-8 place-items-center rounded-lg bg-white/15">
                                glyph(data: iconify_icon!("lucide:folders"), attrs: attributes! { class="size-4" })
                            </span>
                            "Lists, priorities and due dates"
                        </li>
                        <li class="flex items-center gap-3">
                            <span class="grid size-8 place-items-center rounded-lg bg-white/15">
                                glyph(data: iconify_icon!("lucide:search"), attrs: attributes! { class="size-4" })
                            </span>
                            "Search that updates as you type"
                        </li>
                        <li class="flex items-center gap-3">
                            <span class="grid size-8 place-items-center rounded-lg bg-white/15">
                                glyph(data: iconify_icon!("lucide:shield-check"), attrs: attributes! { class="size-4" })
                            </span>
                            "Argon2id passwords and revocable sessions"
                        </li>
                    </ul>
                </div>
            </aside>
            <main class="relative flex items-center justify-center px-4 py-10 sm:px-8">
                <div class="rise w-full max-w-md">
                    <a
                        class="mb-6 flex items-center justify-center gap-2.5 lg:hidden"
                        href=(href!(crate::app::login::page))
                    >
                        <span class="brand-gradient grid size-10 place-items-center rounded-xl text-primary-foreground shadow-sm">
                            glyph(
                                data: iconify_icon!("lucide:check-check"),
                                attrs: attributes! { class="size-5" }
                            )
                        </span>
                        <span class="brand-text text-xl font-bold tracking-tight">"Topcoat To-Do"</span>
                    </a>
                    (child)
                </div>
            </main>
        </div>
    })
}

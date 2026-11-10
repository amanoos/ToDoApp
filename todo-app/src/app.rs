pub(crate) mod account;
pub(crate) mod forgot;
pub(crate) mod healthz;
pub(crate) mod lists;
pub(crate) mod login;
pub(crate) mod logout;
pub(crate) mod register;
pub(crate) mod reset;
pub(crate) mod shell;
pub(crate) mod todos;

use topcoat::{
    Result,
    asset::{AssetBundle, RouterBuilderAssetExt},
    context::Cx,
    cookie::CookieLayer,
    font::{Font, fontsource::fontsource_font},
    router::{
        RouterBuilder, RouterBuilderDiscoverExt, Slot, TrustedProxies,
        error::{SeeOther, see_other},
        href, layout, module_router, route,
    },
    runtime::RuntimeLayer,
    session::SessionLifetime,
    tailwind,
    view::{View, view},
};

use crate::config::Config;
use crate::features::{forgot::EmailSender, throttle::Throttle, user::SESSION_LIFETIME};
use crate::security::SecurityHeaders;

/// The Geist font family from Fontsource, bundled into the app and served from our own
/// origin (`host: Asset`) instead of loaded from a CDN. That keeps visitors' browsers
/// from contacting a third party, avoids an unpinned `@latest` download at page load,
/// and lets the Content-Security-Policy say `font-src 'self'`.
const GEIST: Font = fontsource_font!(GEIST, weight: [400, 700], style: Normal, host: Asset);

/// Builds the application's router.
pub fn router(config: &Config, mail: Option<EmailSender>) -> RouterBuilder {
    // The pages and layouts of this module and its submodules, with paths derived
    // from the module tree.
    let builder = module_router!()
        // The runtime's procedures and shards, which have no place in the module tree.
        .discover()
        .assets(AssetBundle::load().unwrap())
        // Session tokens travel in a cookie; sessions last two weeks.
        .layer(CookieLayer::new())
        .with(SessionLifetime(SESSION_LIFETIME))
        // Remembers recent failed logins in memory, to slow down password guessing. It is
        // keyed by client address, so behind a reverse proxy set `TRUSTED_PROXIES` or
        // `TRUSTED_PROXY_HOPS` (below); without that every request appears to come from
        // the proxy, and all users share one limit.
        .with(Throttle::new())
        // Who may sign up; see `config::Registration`.
        .with(config.registration)
        // Security headers on every response, errors included; see `security`.
        .layer(SecurityHeaders::from_env())
        // Lets the browser runtime rerun pages; register it after other layers.
        .layer(RuntimeLayer::new());

    // Present only when `SMTP_URL` is set; without it the "forgot your password?" form
    // explains that the operator has to supply reset links.
    let builder = match mail {
        Some(sender) => builder.with(sender),
        None => builder,
    };

    // Read the client's address from the proxy's `X-Forwarded-For` header, but only from
    // proxies the deployment names: a client can send that header itself.
    if config.trusted_proxies.is_empty() && config.trusted_proxy_hops == 0 {
        return builder;
    }
    let mut trusted = TrustedProxies::new();
    if !config.trusted_proxies.is_empty() {
        trusted = trusted.networks(config.trusted_proxies.iter().map(String::as_str));
    }
    if config.trusted_proxy_hops > 0 {
        trusted = trusted.nearest(config.trusted_proxy_hops);
    }
    builder.trusted_proxies(trusted)
}

#[layout]
async fn root(cx: &Cx, slot: Slot<'_>) -> Result<impl View> {
    Ok(view! {
        <!DOCTYPE html>
        <html lang="en">
            <head>
                <meta charset="utf-8">
                <meta name="viewport" content="width=device-width, initial-scale=1">
                <title>"Topcoat To-Do"</title>
                <meta name="theme-color" content="#0e7490">
                <link rel="stylesheet" href=(tailwind::stylesheet!())>
                topcoat::font::link(font: GEIST)
                topcoat::runtime::script()
                topcoat::dev::script()
            </head>
            <body>(slot)</body>
        </html>
    })
}

/// The front door: send everyone to the todo list, which in turn sends logged-out visitors
/// to the sign-in page.
#[route(GET)]
async fn home(cx: &Cx) -> Result<SeeOther> {
    Ok(see_other(href!(todos::page).resolve(cx)))
}

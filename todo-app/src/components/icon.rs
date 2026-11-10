use topcoat::{
    Result,
    context::Cx,
    icon::IconData,
    view::{Attributes, View, component, view},
};

/// An icon, rendered as an inline `<svg>` that is `1em` square unless a class sizes it.
///
/// This is `topcoat::icon::icon` without its inline `style="vertical-align: ..."`
/// attribute. This app's Content-Security-Policy allows no inline styles (`style-src
/// 'self'`), so browsers refuse that attribute and log a violation for every icon on the
/// page. Lay icons out with flexbox (`inline-flex items-center gap-2`) instead.
///
/// Icons are decorative: they are hidden from assistive technology, so give the control
/// around them a text label or an `aria-label`.
#[component]
pub async fn glyph(
    cx: &Cx,
    /// The icon to render, from `iconify_icon!`.
    data: IconData,
    /// Extra attributes for the `<svg>` element, such as a size class.
    #[default]
    attrs: Attributes,
) -> Result<impl View> {
    Ok(view! {
        <svg
            viewBox=(data.view_box())
            width="1em"
            height="1em"
            aria-hidden="true"
            (attrs)
        >
            (data.into_body())
        </svg>
    })
}

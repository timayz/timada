//! `/admin/companies`: every company on the platform, for administrators,
//! who release a SIREN registered by someone it does not belong to.

pub mod release;

use timada_tenancy::all_companies;
use topcoat::{
    Result,
    context::{Cx, app_context},
    router::{href, page, query_params},
    view::{View, view},
};

use crate::app::admin::require_admin;
use crate::app::auth::Flash;
use crate::db::ReadPool;
use crate::ui::{document, format_time, notice};

#[topcoat::router::query_params(error = bad_request)]
struct CompaniesQuery {
    error: Option<String>,
    message: Option<String>,
}

pub fn admin_companies_url(cx: &Cx, error: Option<&str>, message: Option<&str>) -> String {
    href!(index).query(Flash { error, message }).resolve(cx)
}

#[page]
pub async fn index(cx: &Cx) -> Result<impl View> {
    require_admin(cx).await?;
    let query = query_params::<CompaniesQuery>(cx)?;
    let rows = all_companies(app_context::<ReadPool>(cx), 0, 200).await?;
    Ok(view! {
        document(
            title: "Companies · Administration",
            <header class="mb-8 flex items-center justify-between">
                <h1 class="text-2xl font-semibold">"Companies"</h1>
                <nav class="flex items-center gap-4 text-sm">
                    <a class="underline" href=(href!(crate::app::home))>"Dashboard"</a>
                </nav>
            </header>
            notice(error: query.error.as_deref(), message: query.message.as_deref())
            <section class="rounded border border-neutral-200 bg-white p-4">
                <p class="mb-3 text-sm text-neutral-600">
                    "A SIREN belongs to one company. When someone registered a SIREN that is not theirs, release it: its rightful company can then register it."
                </p>
                <table class="w-full text-left text-sm">
                    <thead>
                        <tr class="border-b border-neutral-200 text-neutral-500">
                            <th class="py-2 font-medium">"Company"</th>
                            <th class="py-2 font-medium">"SIREN"</th>
                            <th class="py-2 font-medium">"Registered"</th>
                            <th class="py-2"></th>
                        </tr>
                    </thead>
                    <tbody class="divide-y divide-neutral-100">
                        for row in &rows {
                            <tr data-admin-company=(&row.siren)>
                                <td class="py-2">(&row.legal_name)</td>
                                <td class="py-2 font-mono">(&row.siren)</td>
                                <td class="py-2"><time datetime=(row.created_at)>(format_time(row.created_at as u64))</time></td>
                                <td class="py-2 text-right">
                                    <form method="post" action=(href!(release::release))>
                                        <input type="hidden" name="siren" value=(&row.siren)>
                                        <button class="text-red-800 underline">"Release the SIREN"</button>
                                    </form>
                                </td>
                            </tr>
                        }
                    </tbody>
                </table>
            </section>
        )
    })
}

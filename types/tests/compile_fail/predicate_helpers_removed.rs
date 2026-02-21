use forge_types::ui::{AsciiOnly, DetailView, SettingsFilterMode};
use forge_types::{ApiUsage, Provider, ToolProviderScope};

fn main() {
    let _ = AsciiOnly::Enabled.is_enabled();
    let _ = DetailView::Hidden.is_visible();
    let _ = SettingsFilterMode::Filtering.is_filtering();
    let _ = ToolProviderScope::AllProviders.allows(Provider::OpenAI);
    let _ = ApiUsage::default().has_data();
}

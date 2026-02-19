use forge_core::DisplayItem;
use forge_engine::App;
use forge_types::Message;

#[derive(Debug, Clone)]
pub enum ContentBlock {
    Thought(String),
    Response(String),
    ToolResult { name: String, content: String },
}

pub fn extract_blocks(app: &App) -> Vec<ContentBlock> {
    let mut blocks = Vec::new();

    for item in app.display_items() {
        if let DisplayItem::Local(msg) = item {
            match msg {
                Message::Thinking(t) => {
                    blocks.push(ContentBlock::Thought(t.content().to_string()));
                }
                Message::Assistant(a) => {
                    blocks.push(ContentBlock::Response(a.content().to_string()));
                }
                Message::ToolResult(r) => {
                    blocks.push(ContentBlock::ToolResult {
                        name: r.tool_name.clone(),
                        content: r.content.clone(),
                    });
                }
                Message::System(_) | Message::User(_) | Message::ToolUse(_) => {}
            }
        }
    }
    blocks
}

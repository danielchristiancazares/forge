use forge_types::ThoughtSignatureState;

fn main() {
    let _ = forge_engine::ToolCallAccumulator {
        id: "call-1".to_string(),
        name: "Read".to_string(),
        arguments_json: String::new(),
        thought_signature: ThoughtSignatureState::Unsigned,
        args_exceeded: false,
    };
}

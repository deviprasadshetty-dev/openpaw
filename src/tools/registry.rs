use super::Tool;
use std::collections::HashMap;
use std::sync::Arc;

pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self {
            tools: HashMap::new(),
        }
    }

    pub fn register<T: Tool + 'static>(&mut self, tool: T) {
        self.tools.insert(tool.name().to_string(), Arc::new(tool));
    }
    
    pub fn register_arc(&mut self, tool: Arc<dyn Tool>) {
        self.tools.insert(tool.name().to_string(), tool);
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    pub fn list(&self) -> Vec<Arc<dyn Tool>> {
        self.tools.values().cloned().collect()
    }
    
    pub fn list_specs(&self) -> Vec<crate::providers::ToolSpec> {
        self.tools.values().map(|t| crate::providers::ToolSpec {
            name: t.name().to_string(),
            description: t.description().to_string(),
            parameters: serde_json::from_str(&t.parameters_json()).unwrap_or(serde_json::json!({})),
        }).collect()
    }
}

use anyhow::Result;
use async_trait::async_trait;
use hyper::{Body, Request};
use regex::Regex;
use serde_json::Value;
use std::any::Any;
use std::sync::RwLock;

use super::{Plugin, PluginAction};

/// 单条重写规则
#[derive(Debug)]
struct RewriteRule {
    /// 匹配模式（正则表达式）
    pattern: Regex,
    /// 替换目标（支持捕获组引用，如 $1, $2）
    replacement: String,
}

/// URL 重写插件
/// 
/// 配置示例:
/// ```json
/// {
///   "type": "rewrite",
///   "rules": [
///     { "pattern": "^/old/(.*)", "replacement": "/new/$1" },
///     { "pattern": "^/api/v1/(.*)", "replacement": "/api/v2/$1" }
///   ]
/// }
/// ```
#[derive(Debug)]
pub struct RewritePlugin {
    rules: RwLock<Vec<RewriteRule>>,
}

impl RewritePlugin {
    pub fn new() -> Self {
        Self {
            rules: RwLock::new(Vec::new()),
        }
    }
}

#[async_trait]
impl Plugin for RewritePlugin {
    fn name(&self) -> &str {
        "rewrite"
    }

    fn on_load(&self, config: &Value) -> Result<(), anyhow::Error> {
        let mut rules = self.rules.write().unwrap();
        rules.clear();

        if let Some(rules_array) = config.get("rules").and_then(|v| v.as_array()) {
            for rule_value in rules_array {
                let pattern_str = rule_value
                    .get("pattern")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing 'pattern' in rewrite rule"))?;

                let replacement = rule_value
                    .get("replacement")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| anyhow::anyhow!("Missing 'replacement' in rewrite rule"))?
                    .to_string();

                let pattern = Regex::new(pattern_str)
                    .map_err(|e| anyhow::anyhow!("Invalid regex pattern '{}': {}", pattern_str, e))?;

                crate::log_info!("Rewrite rule loaded: {} -> {}", pattern_str, replacement);
                rules.push(RewriteRule { pattern, replacement });
            }
        }

        Ok(())
    }

    fn should_handle(&self, req: &Request<Body>) -> bool {
        let path = req.uri().path();
        let rules = self.rules.read().unwrap();
        
        for rule in rules.iter() {
            if rule.pattern.is_match(path) {
                return true;
            }
        }
        false
    }

    async fn handle_request(&self, mut req: Request<Body>) -> Result<PluginAction, anyhow::Error> {
        let original_path = req.uri().path().to_string();
        let query = req.uri().query().unwrap_or("");

        let new_path = {
            let rules = self.rules.read().unwrap();
            let mut path = original_path.clone();

            for rule in rules.iter() {
                if rule.pattern.is_match(&path) {
                    path = rule.pattern.replace(&path, rule.replacement.as_str()).to_string();
                    break; // 只应用第一个匹配的规则
                }
            }
            path
        };

        if new_path != original_path {
            let new_uri_string = if query.is_empty() {
                new_path.clone()
            } else if new_path.contains('?') {
                if new_path.ends_with('?') || new_path.ends_with('&') {
                    format!("{}{}", new_path, query)
                } else {
                    format!("{}&{}", new_path, query)
                }
            } else {
                format!("{}?{}", new_path, query)
            };

            let new_uri = new_uri_string.parse::<hyper::Uri>()
                .map_err(|e| anyhow::anyhow!("Failed to parse new URI '{}': {}", new_uri_string, e))?;

            *req.uri_mut() = new_uri;

            crate::log_debug!("Rewrite: {} -> {}", original_path, new_path);
        }

        Ok(PluginAction::PassThrough(req))
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

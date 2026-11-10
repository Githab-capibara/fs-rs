//! Discovery of the models an agent exposes through ACP.
//!
//! OpenCode publishes the list of its registered models as a session
//! configuration option named `model`. The dashboard asks the agent for that
//! list, shows it to the user and sends the chosen identifier back with
//! `session/set_config_option`.

use agent_client_protocol::v1::schema::{
    SessionConfigId, SessionConfigKind, SessionConfigOption, SessionConfigOptionCategory,
    SessionConfigSelectOptions,
};

/// Identifier of the configuration option that carries the model list.
pub const MODEL_CONFIG_ID: &str = "model";

/// Models that are never offered to the user.
///
/// The local decision model of `fs-rs` is Foq; the agents must not be routed
/// to the retired `oxcoder` build.
pub const FORBIDDEN_MODELS: &[&str] = &["local/oxcoder-9b"];

/// Whether a model identifier is forbidden.
pub fn is_forbidden(id: &str) -> bool {
    FORBIDDEN_MODELS.contains(&id)
}

/// One model the user can choose from.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ModelChoice {
    /// Identifier sent back to the agent, for example `hnc/minimax-m2.1`.
    pub id: String,
    /// Label shown to the user.
    pub name: String,
    /// Optional longer description.
    pub description: Option<String>,
    /// Whether the agent currently uses this model.
    pub current: bool,
}

impl ModelChoice {
    /// Build a choice.
    pub fn new(id: impl Into<String>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: None,
            current: false,
        }
    }

    /// Label used in the model picker.
    pub fn label(&self) -> &str {
        if self.name.is_empty() {
            &self.id
        } else {
            &self.name
        }
    }
}

/// Extract the model list from the session configuration options.
///
/// Returns an empty list when the agent does not expose a model selector.
pub fn models_from_options(options: &[SessionConfigOption]) -> Vec<ModelChoice> {
    let Some(option) = options.iter().find(|option| is_model_option(option)) else {
        return Vec::new();
    };
    let SessionConfigKind::Select(select) = &option.kind else {
        return Vec::new();
    };
    let current = select.current_value.to_string();
    let mut models = Vec::new();
    let mut push = |value: &agent_client_protocol::v1::schema::SessionConfigSelectOption| {
        let id = value.value.to_string();
        if id.is_empty() || is_forbidden(&id) {
            return;
        }
        models.push(ModelChoice {
            current: id == current,
            id,
            name: value.name.clone(),
            description: value.description.clone(),
        });
    };
    match &select.options {
        SessionConfigSelectOptions::Ungrouped(entries) => {
            for entry in entries {
                push(entry);
            }
        }
        SessionConfigSelectOptions::Grouped(groups) => {
            for group in groups {
                for entry in &group.options {
                    push(entry);
                }
            }
        }
        _ => {}
    }
    models
}

/// Whether the configuration option is the model selector.
fn is_model_option(option: &SessionConfigOption) -> bool {
    if option.id.to_string() == MODEL_CONFIG_ID {
        return true;
    }
    matches!(option.category, Some(SessionConfigOptionCategory::Model))
}

/// The identifier of the model configuration option, when the agent has one.
pub fn model_config_id(options: &[SessionConfigOption]) -> Option<SessionConfigId> {
    options
        .iter()
        .find(|option| is_model_option(option))
        .map(|option| option.id.clone())
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_client_protocol::v1::schema::{
        SessionConfigSelect, SessionConfigSelectOption, SessionConfigValueId,
    };

    fn model_option() -> SessionConfigOption {
        SessionConfigOption::select(
            "model",
            "Model",
            "hnc/model-a",
            vec![
                SessionConfigSelectOption::new("hnc/model-a", "Model A"),
                SessionConfigSelectOption::new("opencode/model-b", "Model B"),
            ],
        )
    }

    #[test]
    fn the_model_list_is_extracted() {
        let models = models_from_options(&[model_option()]);
        assert_eq!(models.len(), 2);
        assert_eq!(models[0].id, "hnc/model-a");
        assert_eq!(models[0].name, "Model A");
        assert!(models[0].current);
        assert!(!models[1].current);
    }

    #[test]
    fn no_model_option_yields_an_empty_list() {
        let options = vec![SessionConfigOption::boolean("thinking", "Thinking", true)];
        assert!(models_from_options(&options).is_empty());
    }

    #[test]
    fn grouped_options_are_flattened() {
        let group = agent_client_protocol::v1::schema::SessionConfigSelectGroup::new(
            "g1",
            "Group",
            vec![SessionConfigSelectOption::new("x/1", "One")],
        );
        let mut option = model_option();
        option.kind = SessionConfigKind::Select(SessionConfigSelect::new(
            SessionConfigValueId::new("x/1"),
            SessionConfigSelectOptions::Grouped(vec![group]),
        ));
        let models = models_from_options(&[option]);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "x/1");
        assert!(models[0].current);
    }

    #[test]
    fn the_config_id_is_found() {
        assert_eq!(
            model_config_id(&[model_option()]).map(|id| id.to_string()),
            Some("model".to_string())
        );
    }

    #[test]
    fn a_choice_without_a_name_falls_back_to_its_id() {
        let choice = ModelChoice::new("x/1", "");
        assert_eq!(choice.label(), "x/1");
    }

    #[test]
    fn forbidden_models_are_never_offered() {
        assert!(is_forbidden("local/oxcoder-9b"));
        assert!(!is_forbidden("hnc/MiniMax-M3.1-Flash"));
    }

    #[test]
    fn the_forbidden_model_is_filtered_out_of_the_list() {
        let option = SessionConfigOption::select(
            "model",
            "Model",
            "local/oxcoder-9b",
            vec![
                SessionConfigSelectOption::new("local/oxcoder-9b", "OxCoder"),
                SessionConfigSelectOption::new("hnc/good", "Good"),
            ],
        );
        let models = models_from_options(&[option]);
        assert_eq!(models.len(), 1);
        assert_eq!(models[0].id, "hnc/good");
    }
}

#[derive(Clone, Copy)]
pub(crate) enum ConfigEdit<'a> {
    Theme(&'a str),
    StatusIndicators(super::StatusIndicatorStyle),
    Sound(bool),
    ToastDelivery(super::ToastDelivery),
    SidebarPanelBorders(bool),
    SidebarAgentDetail(super::AgentDetailConfig),
    SidebarShowAgentPath(bool),
}

impl ConfigEdit<'_> {
    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::Theme(_) => "theme",
            Self::StatusIndicators(_) => "status indicators",
            Self::Sound(_) => "sound setting",
            Self::ToastDelivery(_) => "toast setting",
            Self::SidebarPanelBorders(_)
            | Self::SidebarAgentDetail(_)
            | Self::SidebarShowAgentPath(_) => "sidebar setting",
        }
    }

    pub(crate) fn apply(self, content: &str) -> String {
        match self {
            Self::Theme(name) => {
                let content =
                    super::upsert_section_value(content, "theme", "name", &format!("\"{name}\""));
                super::upsert_section_bool(&content, "theme", "auto_switch", false)
            }
            Self::StatusIndicators(style) => super::upsert_section_value(
                content,
                "ui",
                "status_indicators",
                &format!("\"{}\"", style.as_str()),
            ),
            Self::Sound(enabled) => {
                super::upsert_section_bool(content, "ui.sound", "enabled", enabled)
            }
            Self::ToastDelivery(delivery) => {
                let value = match delivery {
                    super::ToastDelivery::Off => "\"off\"",
                    super::ToastDelivery::Herdr => "\"herdr\"",
                    super::ToastDelivery::Terminal => "\"terminal\"",
                    super::ToastDelivery::System => "\"system\"",
                };
                let content = super::upsert_section_value(content, "ui.toast", "delivery", value);
                super::remove_section_key(&content, "ui.toast", "enabled")
            }
            Self::SidebarPanelBorders(enabled) => {
                super::upsert_section_bool(content, "ui.sidebar", "panel_borders", enabled)
            }
            Self::SidebarAgentDetail(detail) => {
                let value = match detail {
                    super::AgentDetailConfig::Compact => "\"compact\"",
                    super::AgentDetailConfig::Detailed => "\"detailed\"",
                };
                super::upsert_section_value(content, "ui.sidebar", "agent_detail", value)
            }
            Self::SidebarShowAgentPath(shown) => {
                super::upsert_section_bool(content, "ui.sidebar", "show_agent_path", shown)
            }
        }
    }
}

pub(crate) fn update_file_at(
    path: &std::path::Path,
    description: &str,
    update: impl FnOnce(&str) -> String,
) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("failed to create config directory: {error}"))?;
    }
    let content = match super::io::read_optional_config(path) {
        Ok(Some(content)) => content,
        Ok(None) => String::new(),
        Err(error) => {
            return Err(format!(
                "failed to read config before saving {description}: {error}"
            ));
        }
    };
    std::fs::write(path, update(&content))
        .map_err(|error| format!("failed to save {description}: {error}"))
}

pub(crate) fn write_edit(edit: ConfigEdit<'_>) -> Result<(), String> {
    update_file_at(&super::config_path(), edit.description(), |content| {
        edit.apply(content)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SIDEBAR_FIXTURE: &str = r#"# my herdr config
[ui]
sidebar_width = 30

# agent rows
[ui.sidebar.agents]
row_gap = 1
rows = [["state_icon", "agent"]]

[ui.sidebar.agents.rows_by_agent]
claude = [["agent"]]

[ui.toast]
delivery = "herdr"
"#;

    fn apply_all(content: &str, edits: &[ConfigEdit<'_>]) -> String {
        edits
            .iter()
            .fold(content.to_owned(), |content, edit| edit.apply(&content))
    }

    #[test]
    fn sidebar_edits_write_ui_sidebar_without_touching_subtables() {
        let written = apply_all(
            SIDEBAR_FIXTURE,
            &[
                ConfigEdit::SidebarPanelBorders(false),
                ConfigEdit::SidebarAgentDetail(crate::config::AgentDetailConfig::Compact),
                ConfigEdit::SidebarShowAgentPath(false),
            ],
        );

        let config: crate::config::Config = toml::from_str(&written).unwrap();
        let sidebar = &config.ui.sidebar;
        assert!(!sidebar.panel_borders);
        assert_eq!(
            sidebar.agent_detail,
            crate::config::AgentDetailConfig::Compact
        );
        assert!(!sidebar.show_agent_path);
        assert_eq!(sidebar.agents.row_gap, 1);
        assert_eq!(sidebar.agents.rows.len(), 1);
        assert!(sidebar.agents.rows_by_agent.contains_key("claude"));
        assert_eq!(config.ui.sidebar_width, 30);
        assert_eq!(
            config.ui.toast.delivery,
            crate::config::ToastDelivery::Herdr
        );
        assert!(written.starts_with("# my herdr config\n"));
        assert!(written.contains("# agent rows\n[ui.sidebar.agents]\n"));
        assert_eq!(written.matches("[ui.sidebar]").count(), 1);
    }

    #[test]
    fn sidebar_edits_replace_existing_keys_in_place() {
        let content = "[ui.sidebar]\n# keep me\npanel_borders = false\nagent_detail = \"compact\"\nshow_agent_path = false\n\n[ui.sidebar.spaces]\nrow_gap = 1\n";
        let written = apply_all(
            content,
            &[
                ConfigEdit::SidebarPanelBorders(true),
                ConfigEdit::SidebarAgentDetail(crate::config::AgentDetailConfig::Detailed),
                ConfigEdit::SidebarShowAgentPath(true),
            ],
        );

        assert_eq!(
            written,
            "[ui.sidebar]\n# keep me\npanel_borders = true\nagent_detail = \"detailed\"\nshow_agent_path = true\n\n[ui.sidebar.spaces]\nrow_gap = 1\n"
        );
        let config: crate::config::Config = toml::from_str(&written).unwrap();
        assert_eq!(config.ui.sidebar.spaces.row_gap, 1);
    }

    #[test]
    fn update_file_at_does_not_move_a_leading_bom_into_the_file() {
        let dir = std::env::temp_dir().join(format!("herdr-config-bom-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("config.toml");
        std::fs::write(
            &path,
            b"\xEF\xBB\xBF[terminal]\ndefault_shell = \"pwsh.exe\"\n",
        )
        .unwrap();

        update_file_at(&path, "onboarding setting", |content| {
            crate::config::upsert_top_level_bool(content, "onboarding", false)
        })
        .unwrap();

        let written = std::fs::read_to_string(&path).unwrap();
        let _ = std::fs::remove_dir_all(dir);

        assert!(
            !written.contains('\u{feff}'),
            "unexpected BOM in {written:?}"
        );
        assert!(
            toml::from_str::<toml::Value>(&written).is_ok(),
            "written config is not valid TOML: {written:?}"
        );
    }
}

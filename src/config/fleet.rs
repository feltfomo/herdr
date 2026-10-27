use serde::Deserialize;

/// Combined-window machines are a closed, small set: each one runs a discovery loop and owns a
/// sidebar tree, so the limit bounds background SSH work rather than layout.
pub(crate) const MAX_FLEET_MACHINES: usize = 8;
const MAX_MACHINE_ID_BYTES: usize = 64;

#[derive(Debug, Clone, Default, Deserialize)]
#[serde(default)]
pub struct FleetConfig {
    /// Machines that combined windows discover and switch between. Empty disables them.
    pub machines: Vec<FleetMachineConfig>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct FleetMachineConfig {
    /// Stable machine identifier; the local machine is the one matching this host's short name.
    pub id: String,
    /// Sidebar label. Defaults to the identifier.
    pub label: Option<String>,
    /// SSH target. Ignored for the local machine, which always connects over local IPC.
    pub target: String,
}

impl FleetMachineConfig {
    pub fn label(&self) -> &str {
        self.label.as_deref().unwrap_or(&self.id)
    }
}

impl FleetConfig {
    pub fn is_configured(&self) -> bool {
        !self.machines.is_empty()
    }

    pub fn validate(&self) -> Result<(), String> {
        if self.machines.len() > MAX_FLEET_MACHINES {
            return Err(format!(
                "fleet.machines lists {} machines; at most {MAX_FLEET_MACHINES} are supported",
                self.machines.len()
            ));
        }
        let mut ids = std::collections::HashSet::new();
        for machine in &self.machines {
            validate_machine(&machine.id, machine.label(), Some(&machine.target))
                .map_err(|error| format!("fleet machine {:?}: {error}", machine.id))?;
            if !ids.insert(machine.id.as_str()) {
                return Err(format!("fleet machine {:?} is listed twice", machine.id));
            }
        }
        Ok(())
    }

    pub fn diagnostics(&self) -> Option<String> {
        self.validate()
            .err()
            .map(|error| format!("{error}; opening ordinary windows"))
    }

    /// The configured machine this host is, by the first DNS label of its node name, so a
    /// shared config works on every machine regardless of search domains.
    pub fn local_machine(&self, hostname: Option<&str>) -> Option<&FleetMachineConfig> {
        let short = hostname?
            .split('.')
            .next()
            .filter(|name| !name.is_empty())?;
        self.machines.iter().find(|machine| machine.id == short)
    }
}

/// The identity rules shared by `[fleet]` and each window's fleet.json, so a machine accepted
/// in config can never produce a window configuration that Herdr later refuses to load.
/// `target` is `None` for the machine reached over local IPC.
pub(crate) fn validate_machine(id: &str, label: &str, target: Option<&str>) -> Result<(), String> {
    if id.is_empty()
        || id.len() > MAX_MACHINE_ID_BYTES
        || !id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"._-".contains(&b))
    {
        return Err(format!(
            "machine ids use 1-{MAX_MACHINE_ID_BYTES} ASCII letters, digits, '.', '_' or '-'"
        ));
    }
    if label.trim().is_empty()
        || label.len() > crate::client::endpoint::MAX_LABEL_BYTES
        || label.chars().any(char::is_control)
    {
        return Err(format!(
            "machine labels must be non-empty, at most {} bytes and free of control characters",
            crate::client::endpoint::MAX_LABEL_BYTES
        ));
    }
    if let Some(target) = target {
        crate::client::endpoint::SavedSshEndpoint::new(label, target, "default")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(toml: &str) -> FleetConfig {
        toml::from_str::<crate::config::Config>(toml)
            .expect("config parses")
            .fleet
    }

    #[test]
    fn absent_or_empty_fleet_is_unconfigured() {
        assert!(!parse("").is_configured());
        assert!(!parse("[fleet]\n").is_configured());
        assert!(!parse("[fleet]\nmachines = []\n").is_configured());
        assert_eq!(parse("").diagnostics(), None);
    }

    #[test]
    fn machines_parse_with_label_defaulting_to_id() {
        let fleet = parse(
            "[[fleet.machines]]\nid = \"khion\"\ntarget = \"feltfomo@khion\"\n\
             [[fleet.machines]]\nid = \"lumi\"\nlabel = \"laptop\"\ntarget = \"feltfomo@lumi\"\n",
        );
        assert!(fleet.is_configured());
        assert_eq!(fleet.diagnostics(), None);
        assert_eq!(fleet.machines[0].label(), "khion");
        assert_eq!(fleet.machines[1].label(), "laptop");
    }

    #[test]
    fn invalid_machines_produce_a_fallback_diagnostic() {
        let machine = |id: &str, label: Option<&str>, target: &str| FleetMachineConfig {
            id: id.into(),
            label: label.map(str::to_owned),
            target: target.into(),
        };
        let valid = machine("khion", None, "feltfomo@khion");
        for machines in [
            vec![valid.clone(), valid.clone()],
            vec![machine("../khion", None, "feltfomo@khion")],
            vec![machine("", None, "feltfomo@khion")],
            vec![machine(&"x".repeat(65), None, "feltfomo@khion")],
            vec![machine("khion", Some(" "), "feltfomo@khion")],
            vec![machine("khion", Some("bad\nlabel"), "feltfomo@khion")],
            vec![machine("khion", None, "-oProxyCommand=bad")],
            vec![machine("khion", None, "")],
            vec![machine("khion", None, "user:password@khion")],
            vec![valid.clone(); MAX_FLEET_MACHINES + 1],
        ] {
            let fleet = FleetConfig { machines };
            let diagnostic = fleet.diagnostics().expect("rejected");
            assert!(diagnostic.contains("ordinary windows"), "{diagnostic}");
        }
    }

    #[test]
    fn invalid_fleet_is_a_config_diagnostic_not_a_parse_failure() {
        let config: crate::config::Config = toml::from_str(
            "[ui]\nagent_panel_sort = \"spaces\"\n\
             [[fleet.machines]]\nid = \"../bad\"\ntarget = \"feltfomo@khion\"\n",
        )
        .expect("config parses");
        assert!(config.fleet.is_configured());
        let diagnostics = config.collect_diagnostics();
        assert_eq!(
            diagnostics
                .iter()
                .filter(|d| d.contains("ordinary windows"))
                .count(),
            1,
            "{diagnostics:?}"
        );
    }

    #[test]
    fn local_machine_is_the_first_label_of_the_node_name() {
        let fleet = parse(
            "[[fleet.machines]]\nid = \"khion\"\ntarget = \"feltfomo@khion\"\n\
             [[fleet.machines]]\nid = \"lumi\"\ntarget = \"feltfomo@lumi\"\n",
        );
        let local = |hostname| fleet.local_machine(hostname).map(|m| m.id.as_str());
        assert_eq!(local(Some("lumi")), Some("lumi"));
        assert_eq!(local(Some("khion.tail1234.ts.net")), Some("khion"));
        assert_eq!(local(Some("Khion")), None, "identifiers are case-sensitive");
        assert_eq!(local(Some("other")), None);
        assert_eq!(local(Some(".khion")), None);
        assert_eq!(local(None), None);
    }
}

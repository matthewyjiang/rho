#[derive(Clone, Debug, PartialEq)]
pub(super) struct TurnPrompt {
    pub(super) model: String,
    pub(super) display: String,
    pub(super) history: String,
    pub(super) persisted_display: Option<String>,
    /// Skills loaded before the first model request, in order, as one batch
    /// of host-requested `skill` calls.
    pub(super) skills: Vec<String>,
}

impl TurnPrompt {
    pub(super) fn standard(model: String, display: String) -> Self {
        Self {
            history: model.clone(),
            model,
            display,
            persisted_display: None,
            skills: Vec::new(),
        }
    }

    pub(super) fn command(model: String, command: String) -> Self {
        Self {
            model,
            display: command.clone(),
            history: command.clone(),
            persisted_display: Some(command),
            skills: Vec::new(),
        }
    }

    pub(super) fn with_skill(mut self, name: String) -> Self {
        self.skills.push(name);
        self
    }

    /// Also loads every known skill the model text mentions as `$<name>`,
    /// after the skills already listed.
    pub(super) fn load_mentioned_skills(
        &mut self,
        discover: impl FnOnce() -> Vec<crate::skills::Skill>,
    ) {
        let leading = self.skills.iter().map(String::as_str).collect::<Vec<_>>();
        let skills =
            crate::skills::mention::skills_to_load(&leading, &[self.model.as_str()], discover)
                .into_iter()
                .map(str::to_owned)
                .collect();
        self.skills = skills;
    }
}

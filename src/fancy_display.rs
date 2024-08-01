use console::StyledObject;

use pixi_consts::consts;

pub trait FancyDisplay {
    fn fancy_display(&self) -> StyledObject<&str>;
}

impl FancyDisplay for pixi_manifest::EnvironmentName {
    fn fancy_display(&self) -> StyledObject<&str> {
        consts::ENVIRONMENT_STYLE.apply_to(self.as_str())
    }
}

impl FancyDisplay for &pixi_manifest::EnvironmentName {
    fn fancy_display(&self) -> StyledObject<&str> {
        consts::ENVIRONMENT_STYLE.apply_to(self.as_str())
    }
}

impl FancyDisplay for pixi_manifest::TaskName {
    fn fancy_display(&self) -> StyledObject<&str> {
        consts::TASK_STYLE.apply_to(self.as_str())
    }
}

impl FancyDisplay for pixi_manifest::FeatureName {
    fn fancy_display(&self) -> StyledObject<&str> {
        consts::FEATURE_STYLE.apply_to(self.as_str())
    }
}

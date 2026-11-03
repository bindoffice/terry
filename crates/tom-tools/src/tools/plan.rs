//! `update_plan` — a structured plan the model edits instead of re-deriving.
//!
//! Codex exposes the plan as *state* (`update_plan`), not as free text in the
//! system prompt. tom historically only had the prompt rule ("say which step you
//! are on"), and the forensics doc records a 112-turn repeated-thinking incident
//! whose root cause was exactly that: the plan lived only in prose, fell out of
//! the window, and the model rebuilt it from scratch. A tool call + result pair
//! is a plain transcript entry, so the plan is visible to the model on every
//! subsequent turn and survives until compaction — and, because it is a tool
//! result rather than prose, it is cheap to re-assert.
//!
//! The plan is also kept in memory so a front end can render it. It is *not*
//! durable across process restarts in this version; the transcript entry is the
//! durable record.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use tom_agent::agent_tool::AgentTool;
use tom_agent::error::AgentError;
use tom_agent::types::{AgentToolResult, TextContentOrImage, ToolResultPartial};
use tom_ai::types::{ConstrainedSamplingConfig, ConstrainedStrictness, Tool};

pub const UPDATE_PLAN_TOOL_NAME: &str = "update_plan";

/// One step's state. Wire spelling is `pending` / `in_progress` / `completed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum PlanStatus {
    Pending,
    InProgress,
    Completed,
}

impl PlanStatus {
    fn marker(self) -> &'static str {
        match self {
            PlanStatus::Pending => "[ ]",
            PlanStatus::InProgress => "[~]",
            PlanStatus::Completed => "[x]",
        }
    }
}

/// A plan step as stored + rendered.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PlanStep {
    pub step: String,
    pub status: PlanStatus,
}

/// The current plan.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Plan {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub explanation: Option<String>,
    pub steps: Vec<PlanStep>,
}

impl Plan {
    /// Just the checklist, without the summary line. This is what a plan panel
    /// renders, so it must not repeat the "Plan updated. N/M complete" preamble.
    pub fn render_steps(&self) -> String {
        self.steps
            .iter()
            .map(|step| format!("{} {}", step.status.marker(), step.step))
            .collect::<Vec<_>>()
            .join("\n")
    }

    /// Render a compact checklist. Kept terse on purpose: the result is sent
    /// back to the model on every call and repeated in the transcript.
    pub fn render(&self) -> String {
        let mut out = String::from("Plan updated.");
        let total = self.steps.len();
        if total > 0 {
            let completed = self
                .steps
                .iter()
                .filter(|step| step.status == PlanStatus::Completed)
                .count();
            let in_progress = self
                .steps
                .iter()
                .filter(|step| step.status == PlanStatus::InProgress)
                .count();
            out.push_str(&format!(
                " {completed}/{total} complete, {in_progress} in progress."
            ));
        }
        if let Some(explanation) = &self.explanation {
            out.push_str(&format!("\n{explanation}"));
        }
        for step in &self.steps {
            out.push_str(&format!("\n{} {}", step.status.marker(), step.step));
        }
        out
    }
}

/// Wire input.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct PlanInput {
    /// Optional note about why the plan changed.
    #[serde(default)]
    pub explanation: Option<String>,
    /// The complete plan. Each call replaces the previous one.
    pub plan: Vec<PlanStepInput>,
}

/// Wire step.
#[derive(Debug, Clone, Deserialize, JsonSchema)]
pub struct PlanStepInput {
    /// One actionable step, in order.
    pub step: String,
    /// `pending`, `in_progress` or `completed`.
    pub status: PlanStatus,
}

/// Callback fired after a successful update, for a front end rendering the plan.
pub type PlanChangeHandler = Arc<dyn Fn(&Plan) + Send + Sync>;

/// The `update_plan` tool.
pub struct UpdatePlanTool {
    schema: Tool,
    plan: Mutex<Plan>,
    on_change: Option<PlanChangeHandler>,
}

impl UpdatePlanTool {
    fn schema() -> Tool {
        let params = schemars::schema_for!(PlanInput);
        Tool {
            name: UPDATE_PLAN_TOOL_NAME.to_string(),
            description: "Record or update the structured plan for the current task. \
                          Call this before starting a multi-step task and after completing \
                          each step, so the plan is part of the transcript instead of being \
                          re-derived from prose. Each call replaces the whole plan: pass every \
                          remaining step with its status (`pending`, `in_progress`, `completed`). \
                          At most one step may be `in_progress`."
                .to_string(),
            parameters: tom_ai::types::Schema::new(
                serde_json::to_value(params).unwrap_or_default(),
            ),
            constrained_sampling: Some(ConstrainedSamplingConfig::JsonSchema {
                strict: ConstrainedStrictness::Prefer,
            }),
        }
    }

    /// The current plan, for a front end.
    pub fn plan(&self) -> Plan {
        self.plan
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    /// Install a listener for plan changes.
    pub fn set_on_change(&mut self, handler: PlanChangeHandler) {
        self.on_change = Some(handler);
    }
}

/// Build the `update_plan` tool.
pub fn create_update_plan_tool() -> Arc<UpdatePlanTool> {
    Arc::new(UpdatePlanTool {
        schema: UpdatePlanTool::schema(),
        plan: Mutex::new(Plan::default()),
        on_change: None,
    })
}

/// Build the `update_plan` tool with a change listener.
pub fn create_update_plan_tool_with(handler: PlanChangeHandler) -> Arc<UpdatePlanTool> {
    Arc::new(UpdatePlanTool {
        schema: UpdatePlanTool::schema(),
        plan: Mutex::new(Plan::default()),
        on_change: Some(handler),
    })
}

#[async_trait]
impl AgentTool for UpdatePlanTool {
    fn schema(&self) -> &Tool {
        &self.schema
    }

    fn label(&self) -> &str {
        UPDATE_PLAN_TOOL_NAME
    }

    async fn execute(
        &self,
        _tool_call_id: &str,
        params: serde_json::Value,
        _signal: CancellationToken,
        _on_update: Arc<dyn Fn(ToolResultPartial) + Send + Sync>,
    ) -> Result<AgentToolResult, AgentError> {
        let input: PlanInput = serde_json::from_value(params).map_err(|error| {
            AgentError::Validation(format!("update_plan input invalid: {error}"))
        })?;
        let in_progress = input
            .plan
            .iter()
            .filter(|step| step.status == PlanStatus::InProgress)
            .count();
        if in_progress > 1 {
            return Err(AgentError::Validation(
                "update_plan: at most one step may be in_progress".to_string(),
            ));
        }
        let plan = Plan {
            explanation: input.explanation,
            steps: input
                .plan
                .into_iter()
                .map(|step| PlanStep {
                    step: step.step,
                    status: step.status,
                })
                .collect(),
        };
        let rendered = plan.render();
        *self.plan.lock().unwrap_or_else(|error| error.into_inner()) = plan.clone();
        if let Some(handler) = &self.on_change {
            handler(&plan);
        }
        // The details carry the rendered checklist under the `kind: "plan"`
        // shape the transcript view renders as a plan panel. Emitting the raw
        // `{explanation, steps}` object instead left the panel unreachable: the
        // model saw a plan, the user saw a generic tool result.
        Ok(AgentToolResult {
            content: vec![TextContentOrImage::text(rendered)],
            details: serde_json::json!({
                "kind": "plan",
                "active": false,
                "plan": plan.render_steps(),
                "explanation": plan.explanation,
                "steps": plan.steps,
            }),
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn run(params: serde_json::Value) -> AgentToolResult {
        let tool = create_update_plan_tool();
        tool.execute(
            "call-1",
            params,
            CancellationToken::new(),
            Arc::new(|_| {}),
        )
        .await
        .expect("update_plan should accept a well-formed plan")
    }

    #[tokio::test]
    async fn details_use_the_shape_the_plan_panel_renders() {
        let result = run(serde_json::json!({
            "explanation": "porting the parser",
            "plan": [
                {"step": "read the old parser", "status": "completed"},
                {"step": "port the tokenizer", "status": "in_progress"},
                {"step": "port the grammar", "status": "pending"}
            ]
        }))
        .await;

        // The transcript view keys off `details.kind == "plan"` and renders
        // `details.plan` verbatim. Emitting `{explanation, steps}` instead left
        // the panel unreachable.
        assert_eq!(result.details["kind"], "plan");
        assert_eq!(result.details["active"], false);
        let plan = result.details["plan"].as_str().expect("plan is a string");
        assert!(plan.contains("[x] read the old parser"), "{plan}");
        assert!(plan.contains("[~] port the tokenizer"), "{plan}");
        assert!(plan.contains("[ ] port the grammar"), "{plan}");
        // The panel gets the checklist only; the "Plan updated. N/M" summary is
        // for the model, not the widget.
        assert!(!plan.contains("Plan updated"), "{plan}");
    }

    #[tokio::test]
    async fn the_model_facing_text_keeps_the_summary() {
        let result = run(serde_json::json!({
            "plan": [
                {"step": "one", "status": "completed"},
                {"step": "two", "status": "pending"}
            ]
        }))
        .await;
        let text = match &result.content[0] {
            TextContentOrImage::Text(t) => t.text.clone(),
            _ => panic!("expected text"),
        };
        assert!(text.contains("1/2 complete"), "{text}");
    }

    #[tokio::test]
    async fn two_in_progress_steps_are_rejected() {
        let tool = create_update_plan_tool();
        let error = tool
            .execute(
                "call-1",
                serde_json::json!({
                    "plan": [
                        {"step": "one", "status": "in_progress"},
                        {"step": "two", "status": "in_progress"}
                    ]
                }),
                CancellationToken::new(),
                Arc::new(|_| {}),
            )
            .await
            .expect_err("two in-progress steps should be refused");
        assert!(error.to_string().contains("at most one step"), "{error}");
    }

    #[tokio::test]
    async fn each_call_replaces_the_whole_plan() {
        let tool = create_update_plan_tool();
        let update = |steps: serde_json::Value| {
            let tool = Arc::clone(&tool);
            async move {
                tool.execute(
                    "call",
                    serde_json::json!({ "plan": steps }),
                    CancellationToken::new(),
                    Arc::new(|_| {}),
                )
                .await
            }
        };
        update(serde_json::json!([
            {"step": "a", "status": "completed"},
            {"step": "b", "status": "pending"}
        ]))
        .await
        .unwrap();
        update(serde_json::json!([
            {"step": "b", "status": "completed"}
        ]))
        .await
        .unwrap();

        let plan = tool.plan();
        assert_eq!(plan.steps.len(), 1, "the previous plan must be replaced");
        assert_eq!(plan.steps[0].step, "b");
    }
}

use crate::config::{AppConfig, EffortLevel, ModelProfile, ToolProtocol};
use crate::context::ProjectContext;
use crate::fullscreen::{FullscreenInput, FullscreenSnapshot, FullscreenUi};
use crate::hooks::HookRunner;
use crate::parser::parse_model_output;
use crate::permissions::{PermissionEngine, PermissionProfile};
use crate::preview::PreviewServer;
use crate::providers::{MessageRole, ModelMessage, ModelRequest, ProviderRegistry};
use crate::session::{self, Session};
use crate::tools::{ToolCall, ToolRegistry, ToolResult};
use crate::ui::{self, FooterInfo, HeaderInfo};
use anyhow::{bail, Result};
use futures_util::future::join_all;
use serde::Serialize;
use serde_json::json;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::process::Command;

pub async fn run_interactive(
    config: AppConfig,
    providers: ProviderRegistry,
    workspace: PathBuf,
) -> Result<()> {
    let context = ProjectContext::load(&workspace)?;
    let session = Session::create()?;
    let permissions = PermissionEngine::new(
        PermissionProfile::parse(&config.permission_profile),
        workspace.clone(),
        true,
    );
    let tools = ToolRegistry::new(
        workspace.clone(),
        permissions,
        HookRunner::new(
            config.hooks.pre_tool.clone(),
            config.hooks.post_tool.clone(),
        ),
    );
    let mut state = AgentState::new(config, context, session);
    state.append_metadata()?;

    if should_use_fullscreen() {
        return run_fullscreen_loop(&mut state, &providers, &tools).await;
    }

    render_header(&state);

    loop {
        print!("{}", ui::prompt_box(state.work_mode.as_str()));
        io::stdout().flush()?;
        let mut input = String::new();
        if io::stdin().read_line(&mut input)? == 0 {
            break;
        }
        ui::close_prompt_box();
        let input = input.trim();
        if input.is_empty() {
            continue;
        }
        if input.starts_with('/') {
            if handle_slash_command(input, &mut state, &providers, &tools).await? {
                break;
            }
            render_footer(&state);
            continue;
        }

        let output = run_user_turn(&mut state, &providers, &tools, input, true).await?;
        if !output.trim().is_empty() {
            println!("{output}");
        }
        render_footer(&state);
    }

    Ok(())
}

pub async fn run_resume(
    config: AppConfig,
    providers: ProviderRegistry,
    workspace: PathBuf,
    session_id: String,
) -> Result<()> {
    let context = ProjectContext::load(&workspace)?;
    let session = Session::open(&session_id)?;
    let permissions = PermissionEngine::new(
        PermissionProfile::parse(&config.permission_profile),
        workspace.clone(),
        true,
    );
    let tools = ToolRegistry::new(
        workspace,
        permissions,
        HookRunner::new(
            config.hooks.pre_tool.clone(),
            config.hooks.post_tool.clone(),
        ),
    );
    let mut state = AgentState::new(config, context, session);
    state.restore_transcript()?;

    if should_use_fullscreen() {
        return run_fullscreen_loop(&mut state, &providers, &tools).await;
    }

    render_header(&state);
    loop {
        print!("{}", ui::prompt_box(state.work_mode.as_str()));
        io::stdout().flush()?;
        let mut input = String::new();
        if io::stdin().read_line(&mut input)? == 0 {
            break;
        }
        ui::close_prompt_box();
        let input = input.trim();
        if input.is_empty() {
            continue;
        }
        if input.starts_with('/') {
            if handle_slash_command(input, &mut state, &providers, &tools).await? {
                break;
            }
            render_footer(&state);
            continue;
        }
        let output = run_user_turn(&mut state, &providers, &tools, input, true).await?;
        if !output.trim().is_empty() {
            println!("{output}");
        }
        render_footer(&state);
    }
    Ok(())
}

pub async fn run_exec(
    config: AppConfig,
    providers: ProviderRegistry,
    workspace: PathBuf,
    task: String,
) -> Result<()> {
    let context = ProjectContext::load(&workspace)?;
    let session = Session::create()?;
    let permissions = PermissionEngine::new(
        PermissionProfile::parse(&config.permission_profile),
        workspace.clone(),
        false,
    );
    let tools = ToolRegistry::new(
        workspace,
        permissions,
        HookRunner::new(
            config.hooks.pre_tool.clone(),
            config.hooks.post_tool.clone(),
        ),
    );
    let mut state = AgentState::new(config, context, session);
    state.append_metadata()?;
    let output = run_agent_turn(
        &mut state,
        &providers,
        &tools,
        &task,
        false,
        TurnMode::Implement,
    )
    .await?;
    println!("{output}");
    eprintln!("session: {}", state.session.path().display());
    Ok(())
}

async fn run_fullscreen_loop(
    state: &mut AgentState,
    providers: &ProviderRegistry,
    tools: &ToolRegistry,
) -> Result<()> {
    let mut screen = FullscreenUi::enter()?;
    screen.push_system("Ready. Type /help for commands. Ctrl+C or /exit leaves ClaudeCodeX.");

    loop {
        screen.set_status("idle");
        match screen.read_input(&fullscreen_snapshot(state))? {
            FullscreenInput::Exit => break,
            FullscreenInput::Submit(input) => {
                if input == "/clear" {
                    screen.clear_entries();
                    continue;
                }
                screen.push_user(&input);
                if input.starts_with('/') {
                    if handle_fullscreen_slash_command(&input, state, providers, tools, &mut screen)
                        .await?
                    {
                        break;
                    }
                    continue;
                }

                screen.set_status("thinking");
                screen.draw(&fullscreen_snapshot(state))?;
                match run_user_turn(state, providers, tools, &input, false).await {
                    Ok(output) => {
                        if output.trim().is_empty() {
                            screen.push_assistant("Done.");
                        } else {
                            screen.push_assistant(output);
                        }
                    }
                    Err(error) => {
                        screen.push_system(format!("error: {error:#}"));
                    }
                }
            }
        }
    }

    Ok(())
}

async fn handle_fullscreen_slash_command(
    input: &str,
    state: &mut AgentState,
    providers: &ProviderRegistry,
    tools: &ToolRegistry,
    screen: &mut FullscreenUi,
) -> Result<bool> {
    let mut parts = input.split_whitespace();
    let command = parts.next().unwrap_or_default();
    match command {
        "/exit" | "/quit" => Ok(true),
        "/help" => {
            screen.push_system(fullscreen_help());
            Ok(false)
        }
        "/context" => {
            screen.push_system(format!(
                "{}\n{}",
                state.context.summary(),
                state.context.render_for_prompt()
            ));
            Ok(false)
        }
        "/providers" => {
            screen.push_system(
                providers
                    .summaries()
                    .into_iter()
                    .map(|summary| summary.to_string())
                    .collect::<Vec<_>>()
                    .join("\n"),
            );
            Ok(false)
        }
        "/effort" => {
            match parts.next() {
                Some(value) => {
                    let parsed: EffortLevel = value.parse()?;
                    state.effort_override = Some(parsed);
                    state.session.append(
                        "effort",
                        EventText {
                            text: parsed.as_str(),
                        },
                    )?;
                    screen.push_system(format!("effort: {parsed}"));
                }
                None => screen.push_system(format!("effort: {}", state.current_effort())),
            }
            Ok(false)
        }
        "/model" => {
            let provider = parts.next();
            let model = parts.next();
            match (provider, model) {
                (Some(provider), Some(model)) => {
                    providers.get(provider)?;
                    state.selected_provider = provider.to_string();
                    state.selected_model = model.to_string();
                    screen.push_system(format!(
                        "model: {} {}",
                        state.selected_provider, state.selected_model
                    ));
                }
                _ => screen.push_system(format!(
                    "model: {} {}",
                    state.selected_provider, state.selected_model
                )),
            }
            Ok(false)
        }
        "/permissions" => {
            screen.push_system(format!("permissions: {}", state.config.permission_profile));
            Ok(false)
        }
        "/session" => {
            screen.push_system(state.session.path().display().to_string());
            Ok(false)
        }
        "/compact" => {
            let summary = compact_transcript(&mut state.transcript);
            state
                .session
                .append("compact_summary", EventText { text: &summary })?;
            state.session.append_memory("compact_summary", &summary)?;
            screen.push_system(format!("compacted session context\n{summary}"));
            Ok(false)
        }
        "/plan" => {
            let value = parts.next().unwrap_or("on");
            match value {
                "on" => {
                    state.work_mode = WorkMode::Plan;
                    state.session.append(
                        "work_mode",
                        EventText {
                            text: state.work_mode.as_str(),
                        },
                    )?;
                    screen.push_system("plan mode: on");
                }
                "off" => {
                    state.work_mode = WorkMode::Agent;
                    state.pending_plan = None;
                    state.session.append(
                        "work_mode",
                        EventText {
                            text: state.work_mode.as_str(),
                        },
                    )?;
                    state
                        .session
                        .append("plan_resolved", EventText { text: "off" })?;
                    screen.push_system("plan mode: off");
                }
                "status" => {
                    let pending = state
                        .pending_plan
                        .as_ref()
                        .map(|plan| format!("\npending plan for: {}", plan.task))
                        .unwrap_or_default();
                    screen.push_system(format!("plan mode: {}{pending}", state.work_mode.as_str()));
                }
                other => screen.push_system(format!(
                    "unknown plan option `{other}`; use /plan on, /plan off, or /plan status"
                )),
            }
            Ok(false)
        }
        "/approve" => {
            let Some(plan) = state.pending_plan.take() else {
                screen.push_system("no pending plan");
                return Ok(false);
            };
            state
                .session
                .append("plan_resolved", EventText { text: "approved" })?;
            state.work_mode = WorkMode::Agent;
            state.session.append(
                "work_mode",
                EventText {
                    text: state.work_mode.as_str(),
                },
            )?;
            let approved_task = format!(
                "Implement this approved plan for the original task.\n\nOriginal task:\n{}\n\nApproved plan:\n{}",
                plan.task, plan.plan
            );
            screen.set_status("implementing approved plan");
            screen.draw(&fullscreen_snapshot(state))?;
            match run_agent_turn(
                state,
                providers,
                tools,
                &approved_task,
                false,
                TurnMode::Implement,
            )
            .await
            {
                Ok(output) => screen.push_assistant(if output.trim().is_empty() {
                    "Approved plan implemented.".to_string()
                } else {
                    output
                }),
                Err(error) => screen.push_system(format!("error: {error:#}")),
            }
            Ok(false)
        }
        "/reject" => {
            if state.pending_plan.take().is_some() {
                state
                    .session
                    .append("plan_resolved", EventText { text: "rejected" })?;
                screen.push_system("pending plan discarded");
            } else {
                screen.push_system("no pending plan");
            }
            Ok(false)
        }
        "/status" => {
            screen.push_system(git_status(&state.context.workspace));
            Ok(false)
        }
        "/diff" => {
            screen.push_system(git_diff(&state.context.workspace));
            Ok(false)
        }
        "/review" => {
            screen.set_status("running review");
            screen.draw(&fullscreen_snapshot(state))?;
            screen.push_system(run_ccx_command(&state.context.workspace, &["review"]));
            Ok(false)
        }
        "/skills" => {
            screen.push_system(run_ccx_command(&state.context.workspace, &["skills"]));
            Ok(false)
        }
        "/subagent" => {
            let kind = parts.next().unwrap_or("plan");
            let task = parts.collect::<Vec<_>>().join(" ");
            screen.set_status(format!("subagent {kind}"));
            screen.draw(&fullscreen_snapshot(state))?;
            match crate::subagents::run_subagent(
                &state.config,
                providers,
                state.context.workspace.clone(),
                kind,
                &task,
                false,
            )
            .await
            {
                Ok(report) => {
                    state.session.append("subagent_result", &report)?;
                    state
                        .session
                        .append_memory("subagent_result", &report.report)?;
                    state.transcript.push(ModelMessage {
                        role: MessageRole::System,
                        content: format!(
                            "Subagent `{}` report for `{}`:\n{}",
                            report.kind, report.task, report.report
                        ),
                    });
                    screen.push_system(format!(
                        "subagent: {}  tool calls: {}\n{}",
                        report.kind, report.tool_calls, report.report
                    ));
                }
                Err(error) => screen.push_system(format!("subagent error: {error:#}")),
            }
            Ok(false)
        }
        "/preview" => {
            let path = parts.next().unwrap_or("index.html");
            if state.preview.is_none() {
                state.preview = Some(PreviewServer::start(state.context.workspace.clone())?);
            }
            let url = state.preview.as_ref().unwrap().url_for(path)?;
            screen.push_system(format!("preview  {url}"));
            Ok(false)
        }
        "/mascot" => {
            screen.push_system("The animated crab is shown in the full-screen header and updates while the input box is waiting for keys.");
            Ok(false)
        }
        other => {
            screen.push_system(format!("unknown command `{other}`"));
            Ok(false)
        }
    }
}

fn should_use_fullscreen() -> bool {
    io::stdin().is_terminal() && io::stdout().is_terminal()
}

async fn run_user_turn(
    state: &mut AgentState,
    providers: &ProviderRegistry,
    tools: &ToolRegistry,
    input: &str,
    render_ui: bool,
) -> Result<String> {
    if state.work_mode == WorkMode::Plan {
        return run_plan_mode_turn(state, providers, tools, input, render_ui).await;
    }
    run_agent_turn(
        state,
        providers,
        tools,
        input,
        render_ui,
        TurnMode::Implement,
    )
    .await
}

async fn run_plan_mode_turn(
    state: &mut AgentState,
    providers: &ProviderRegistry,
    tools: &ToolRegistry,
    input: &str,
    render_ui: bool,
) -> Result<String> {
    if state.pending_plan.is_some() {
        return Ok(
            "A plan is already waiting for approval. Use `/approve` to implement it, `/reject` to discard it, or `/plan off` to leave plan mode."
                .to_string(),
        );
    }

    let plan = run_agent_turn(
        state,
        providers,
        tools,
        input,
        render_ui,
        TurnMode::PlanOnly,
    )
    .await?;
    state.pending_plan = Some(PendingPlan {
        task: input.to_string(),
        plan: plan.clone(),
    });
    state.session.append(
        "pending_plan",
        PlanEvent {
            task: input,
            plan: &plan,
        },
    )?;
    Ok(format!(
        "{plan}\n\nPlan saved. Use `/approve` to implement it or `/reject` to discard it."
    ))
}

struct AgentState {
    config: AppConfig,
    context: ProjectContext,
    session: Session,
    selected_provider: String,
    selected_model: String,
    effort_override: Option<EffortLevel>,
    work_mode: WorkMode,
    pending_plan: Option<PendingPlan>,
    transcript: Vec<ModelMessage>,
    preview: Option<PreviewServer>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WorkMode {
    Agent,
    Plan,
}

impl WorkMode {
    fn as_str(self) -> &'static str {
        match self {
            Self::Agent => "agent",
            Self::Plan => "plan",
        }
    }
}

#[derive(Debug, Clone)]
struct PendingPlan {
    task: String,
    plan: String,
}

#[derive(Debug, Clone, Copy)]
enum TurnMode {
    Implement,
    PlanOnly,
}

impl AgentState {
    fn new(config: AppConfig, context: ProjectContext, session: Session) -> Self {
        let selected_provider = config
            .model_profiles
            .get(&config.default_model)
            .and_then(|profile| profile.provider.clone())
            .unwrap_or_else(|| config.default_provider.clone());
        Self {
            selected_provider,
            selected_model: config.default_model.clone(),
            effort_override: None,
            work_mode: WorkMode::Agent,
            pending_plan: None,
            config,
            context,
            session,
            transcript: Vec::new(),
            preview: None,
        }
    }

    fn append_metadata(&self) -> Result<()> {
        self.session.append(
            "metadata",
            json!({
                "provider": &self.selected_provider,
                "model": &self.selected_model,
                "effort": self.current_effort().as_str(),
                "work_mode": self.work_mode.as_str(),
                "permission_profile": &self.config.permission_profile,
                "workspace": &self.context.workspace,
            }),
        )
    }

    fn restore_transcript(&mut self) -> Result<()> {
        for event in session::read_events(&self.session)? {
            match event.kind.as_str() {
                "metadata" => {
                    if let Some(provider) = event.payload.get("provider").and_then(|v| v.as_str()) {
                        self.selected_provider = provider.to_string();
                    }
                    if let Some(model) = event.payload.get("model").and_then(|v| v.as_str()) {
                        self.selected_model = model.to_string();
                    }
                    if let Some(effort) = event.payload.get("effort").and_then(|v| v.as_str()) {
                        self.effort_override = effort.parse().ok();
                    }
                    if let Some(work_mode) = event.payload.get("work_mode").and_then(|v| v.as_str())
                    {
                        self.work_mode = parse_work_mode(work_mode);
                    }
                }
                "effort" => {
                    if let Some(effort) = event.payload.get("text").and_then(|v| v.as_str()) {
                        self.effort_override = effort.parse().ok();
                    }
                }
                "work_mode" => {
                    if let Some(mode) = event.payload.get("text").and_then(|v| v.as_str()) {
                        self.work_mode = parse_work_mode(mode);
                    }
                }
                "pending_plan" => {
                    let task = event
                        .payload
                        .get("task")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default();
                    let plan = event
                        .payload
                        .get("plan")
                        .and_then(|value| value.as_str())
                        .unwrap_or_default();
                    if !task.is_empty() && !plan.is_empty() {
                        self.pending_plan = Some(PendingPlan {
                            task: task.to_string(),
                            plan: plan.to_string(),
                        });
                    }
                }
                "plan_resolved" => {
                    self.pending_plan = None;
                }
                "compact_summary" => {
                    if let Some(text) = event.payload.get("text").and_then(|v| v.as_str()) {
                        self.transcript.push(ModelMessage {
                            role: MessageRole::System,
                            content: format!("Session summary so far:\n{text}"),
                        });
                    }
                }
                "subagent_result" => {
                    let kind = event
                        .payload
                        .get("kind")
                        .and_then(|v| v.as_str())
                        .unwrap_or("subagent");
                    let task = event
                        .payload
                        .get("task")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if let Some(report) = event.payload.get("report").and_then(|v| v.as_str()) {
                        self.transcript.push(ModelMessage {
                            role: MessageRole::System,
                            content: format!("Subagent `{kind}` report for `{task}`:\n{report}"),
                        });
                    }
                }
                "user" => {
                    if let Some(text) = event.payload.get("text").and_then(|v| v.as_str()) {
                        self.transcript.push(ModelMessage {
                            role: MessageRole::User,
                            content: text.to_string(),
                        });
                    }
                }
                "assistant" => {
                    if let Some(text) = event.payload.get("text").and_then(|v| v.as_str()) {
                        self.transcript.push(ModelMessage {
                            role: MessageRole::Assistant,
                            content: text.to_string(),
                        });
                    }
                }
                "tool_result" => {
                    self.transcript.push(ModelMessage {
                        role: MessageRole::Tool,
                        content: event.payload.to_string(),
                    });
                }
                _ => {}
            }
        }
        self.trim_restored_transcript();
        Ok(())
    }

    fn system_messages(&self) -> Vec<ModelMessage> {
        let profile = self.model_profile();
        if profile.tool_protocol == ToolProtocol::SimpleJson {
            return vec![ModelMessage {
                role: MessageRole::System,
                content: local_model_prompt(
                    &self.context.workspace.display().to_string(),
                    &self.context.render_for_prompt(),
                    profile.max_tool_prompt_size,
                ),
            }];
        }

        vec![
            ModelMessage {
                role: MessageRole::System,
                content: base_prompt().to_string(),
            },
            ModelMessage {
                role: MessageRole::System,
                content: format!(
                    "Workspace: {}\n\nProject instructions:\n{}",
                    self.context.workspace.display(),
                    self.context.render_for_prompt()
                ),
            },
            ModelMessage {
                role: MessageRole::System,
                content: ToolRegistry::tool_manifest().to_string(),
            },
        ]
    }

    fn model_profile(&self) -> ModelProfile {
        self.config
            .model_profiles
            .get(&self.selected_model)
            .cloned()
            .unwrap_or_default()
    }

    fn current_effort(&self) -> EffortLevel {
        self.effort_override.unwrap_or_else(|| {
            self.config
                .resolve_effort(&self.selected_provider, &self.selected_model)
        })
    }

    fn trim_restored_transcript(&mut self) {
        const RECENT: usize = 24;
        if self.transcript.len() <= RECENT {
            return;
        }
        let mut durable = self
            .transcript
            .iter()
            .filter(|message| {
                matches!(message.role, MessageRole::System)
                    && (message.content.contains("Session summary")
                        || message.content.contains("Subagent `"))
            })
            .cloned()
            .collect::<Vec<_>>();
        let recent = self.transcript.split_off(self.transcript.len() - RECENT);
        durable.extend(recent);
        self.transcript = durable;
    }
}

async fn run_agent_turn(
    state: &mut AgentState,
    providers: &ProviderRegistry,
    tools: &ToolRegistry,
    user_input: &str,
    render_ui: bool,
    turn_mode: TurnMode,
) -> Result<String> {
    state
        .session
        .append("user", EventText { text: user_input })?;
    state.transcript.push(ModelMessage {
        role: MessageRole::User,
        content: user_input.to_string(),
    });

    let mut final_visible = String::new();
    for _ in 0..state.config.max_agent_turns {
        let (turn_provider, turn_model) = routed_provider_model(state, user_input);
        let provider = providers.get(&turn_provider)?;
        let turn_profile = state
            .config
            .model_profiles
            .get(&turn_model)
            .cloned()
            .unwrap_or_default();
        let turn_effort = auto_effort_for_task(
            state
                .effort_override
                .unwrap_or_else(|| state.config.resolve_effort(&turn_provider, &turn_model)),
            user_input,
        );
        let mut messages = state.system_messages();
        if matches!(turn_mode, TurnMode::PlanOnly) {
            messages.push(ModelMessage {
                role: MessageRole::System,
                content: plan_mode_prompt().to_string(),
            });
        }
        if let Some(relevant_context) = selected_file_context(&state.context.workspace, user_input)
        {
            messages.push(ModelMessage {
                role: MessageRole::System,
                content: relevant_context,
            });
        }
        messages.extend(state.transcript.clone());
        messages = budget_messages(messages, turn_profile.context_budget);
        let request = ModelRequest {
            model: turn_model,
            messages,
            profile: turn_profile,
            effort: turn_effort,
            tools: if matches!(turn_mode, TurnMode::PlanOnly) {
                ToolRegistry::read_only_tool_specs()
            } else {
                ToolRegistry::native_tool_specs()
            },
        };
        let mut streamed_any = false;
        let response_result = if provider.capabilities().streaming {
            let mut streamed_text = String::new();
            let result = provider
                .generate_stream(request, &mut |chunk| {
                    streamed_text.push_str(&chunk);
                    if render_ui {
                        streamed_any = true;
                        ui::render_stream_chunk(&chunk);
                    }
                    !has_complete_streamed_tool_call(&streamed_text)
                })
                .await;
            if streamed_any {
                println!();
            }
            result
        } else {
            let animation = render_ui.then(|| ui::ActivityAnimation::start("thinking"));
            let result = provider.generate(request).await;
            if let Some(animation) = animation {
                animation.stop();
            }
            result
        };
        let response = response_result?;

        state.session.append(
            "assistant",
            EventText {
                text: &response.text,
            },
        )?;
        state.transcript.push(ModelMessage {
            role: MessageRole::Assistant,
            content: response.text.clone(),
        });

        let parsed = parse_model_output(&response.text)?;
        if !parsed.visible_text.trim().is_empty() {
            if !final_visible.is_empty() {
                final_visible.push_str("\n\n");
            }
            final_visible.push_str(parsed.visible_text.trim());
        }

        let mut calls = if response.tool_calls.is_empty() {
            parsed.calls
        } else {
            response.tool_calls
        };
        if calls.is_empty() && matches!(turn_mode, TurnMode::Implement) {
            if let Some(call) = infer_file_write(user_input, &response.text) {
                state.session.append("inferred_tool_call", &call)?;
                calls.push(call);
            }
        }
        if calls.is_empty() {
            return Ok(if streamed_any {
                String::new()
            } else {
                final_visible
            });
        }

        let tool_results =
            execute_tool_calls(state, tools, user_input, calls, render_ui, turn_mode).await?;

        state.transcript.push(ModelMessage {
            role: MessageRole::Tool,
            content: render_tool_results(&tool_results)?,
        });

        if state.model_profile().tool_protocol == ToolProtocol::SimpleJson
            && tool_results
                .iter()
                .any(|result| result.success && result.tool == "write_file")
        {
            let summary = tool_results
                .iter()
                .find(|result| result.success && result.tool == "write_file")
                .map(|result| result.content.clone())
                .unwrap_or_else(|| "write_file completed".to_string());
            return Ok(if final_visible.trim().is_empty() {
                summary
            } else {
                format!("{}\n\n{}", final_visible.trim(), summary)
            });
        }
    }

    bail!(
        "agent reached max turn limit ({}) before completing",
        state.config.max_agent_turns
    )
}

async fn handle_slash_command(
    input: &str,
    state: &mut AgentState,
    providers: &ProviderRegistry,
    tools: &ToolRegistry,
) -> Result<bool> {
    let mut parts = input.split_whitespace();
    let command = parts.next().unwrap_or_default();
    match command {
        "/exit" | "/quit" => Ok(true),
        "/help" => {
            ui::render_grouped_help();
            Ok(false)
        }
        "/clear" => {
            ui::clear_screen()?;
            render_header(state);
            Ok(false)
        }
        "/context" => {
            println!("{}", state.context.summary());
            println!("{}", state.context.render_for_prompt());
            Ok(false)
        }
        "/providers" => {
            for summary in providers.summaries() {
                println!("{summary}");
            }
            Ok(false)
        }
        "/effort" => {
            let effort = parts.next();
            match effort {
                Some(value) => {
                    let parsed: EffortLevel = value.parse()?;
                    state.effort_override = Some(parsed);
                    state.session.append(
                        "effort",
                        EventText {
                            text: parsed.as_str(),
                        },
                    )?;
                    println!("effort: {parsed}");
                }
                None => println!("effort: {}", state.current_effort()),
            }
            Ok(false)
        }
        "/plan" => {
            let value = parts.next().unwrap_or("on");
            match value {
                "on" => {
                    state.work_mode = WorkMode::Plan;
                    state.session.append(
                        "work_mode",
                        EventText {
                            text: state.work_mode.as_str(),
                        },
                    )?;
                    println!("plan mode: on");
                }
                "off" => {
                    state.work_mode = WorkMode::Agent;
                    state.pending_plan = None;
                    state.session.append(
                        "work_mode",
                        EventText {
                            text: state.work_mode.as_str(),
                        },
                    )?;
                    state
                        .session
                        .append("plan_resolved", EventText { text: "off" })?;
                    println!("plan mode: off");
                }
                "status" => {
                    println!("plan mode: {}", state.work_mode.as_str());
                    if let Some(plan) = &state.pending_plan {
                        println!("pending plan for: {}", plan.task);
                    }
                }
                other => {
                    println!(
                        "unknown plan option `{other}`; use /plan on, /plan off, or /plan status"
                    );
                }
            }
            Ok(false)
        }
        "/approve" => {
            let Some(plan) = state.pending_plan.take() else {
                println!("no pending plan");
                return Ok(false);
            };
            state
                .session
                .append("plan_resolved", EventText { text: "approved" })?;
            state.work_mode = WorkMode::Agent;
            state.session.append(
                "work_mode",
                EventText {
                    text: state.work_mode.as_str(),
                },
            )?;
            let approved_task = format!(
                "Implement this approved plan for the original task.\n\nOriginal task:\n{}\n\nApproved plan:\n{}",
                plan.task, plan.plan
            );
            let output = run_agent_turn(
                state,
                providers,
                tools,
                &approved_task,
                true,
                TurnMode::Implement,
            )
            .await?;
            if !output.trim().is_empty() {
                println!("{output}");
            }
            Ok(false)
        }
        "/reject" => {
            if state.pending_plan.take().is_some() {
                state
                    .session
                    .append("plan_resolved", EventText { text: "rejected" })?;
                println!("pending plan discarded");
            } else {
                println!("no pending plan");
            }
            Ok(false)
        }
        "/model" => {
            let provider = parts.next();
            let model = parts.next();
            match (provider, model) {
                (Some(provider), Some(model)) => {
                    providers.get(provider)?;
                    state.selected_provider = provider.to_string();
                    state.selected_model = model.to_string();
                    println!(
                        "model: {} {}",
                        state.selected_provider, state.selected_model
                    );
                }
                _ => println!(
                    "model: {} {}",
                    state.selected_provider, state.selected_model
                ),
            }
            Ok(false)
        }
        "/permissions" => {
            println!("permissions: {}", state.config.permission_profile);
            Ok(false)
        }
        "/session" => {
            println!("{}", state.session.path().display());
            Ok(false)
        }
        "/compact" => {
            let summary = compact_transcript(&mut state.transcript);
            state
                .session
                .append("compact_summary", EventText { text: &summary })?;
            state.session.append_memory("compact_summary", &summary)?;
            println!("compacted session context\n{summary}");
            Ok(false)
        }
        "/mascot" => {
            ui::render_mascot_preview();
            Ok(false)
        }
        "/status" => {
            println!("{}", git_status(&state.context.workspace));
            Ok(false)
        }
        "/review" => {
            crate::review::run_review(&state.context.workspace, &[])?;
            Ok(false)
        }
        "/skills" => {
            crate::skills::print_skills(&state.context.workspace)?;
            Ok(false)
        }
        "/subagent" => {
            let kind = parts.next().unwrap_or("plan");
            let task = parts.collect::<Vec<_>>().join(" ");
            let report = crate::subagents::run_subagent(
                &state.config,
                providers,
                state.context.workspace.clone(),
                kind,
                &task,
                true,
            )
            .await?;
            state.session.append("subagent_result", &report)?;
            state
                .session
                .append_memory("subagent_result", &report.report)?;
            state.transcript.push(ModelMessage {
                role: MessageRole::System,
                content: format!(
                    "Subagent `{}` report for `{}`:\n{}",
                    report.kind, report.task, report.report
                ),
            });
            Ok(false)
        }
        "/diff" => {
            ui::render_diff(&git_diff(&state.context.workspace));
            Ok(false)
        }
        "/preview" => {
            let path = parts.next().unwrap_or("index.html");
            if state.preview.is_none() {
                state.preview = Some(PreviewServer::start(state.context.workspace.clone())?);
            }
            let url = state.preview.as_ref().unwrap().url_for(path)?;
            println!("preview  {url}");
            Ok(false)
        }
        other => {
            println!("unknown command `{other}`");
            Ok(false)
        }
    }
}

fn render_header(state: &AgentState) {
    let session_short = short_session_id(state);
    ui::render_header(HeaderInfo {
        version: env!("CARGO_PKG_VERSION"),
        provider: &state.selected_provider,
        model: &state.selected_model,
        effort: state.current_effort().as_str(),
        permissions: &state.config.permission_profile,
        workspace: &state.context.workspace,
        context_files: state.context.instruction_files.len(),
        session_short: &session_short,
        mode: state.work_mode.as_str(),
    });
}

fn fullscreen_snapshot(state: &AgentState) -> FullscreenSnapshot {
    FullscreenSnapshot {
        version: env!("CARGO_PKG_VERSION").to_string(),
        provider: state.selected_provider.clone(),
        model: state.selected_model.clone(),
        effort: state.current_effort().as_str().to_string(),
        permissions: state.config.permission_profile.clone(),
        mode: state.work_mode.as_str().to_string(),
        workspace: state.context.workspace.display().to_string(),
        branch: git_branch(&state.context.workspace),
        repo_state: git_repo_state(&state.context.workspace),
        session: short_session_id(state),
        context_files: state.context.instruction_files.len(),
    }
}

fn fullscreen_help() -> &'static str {
    r#"Session
  /session       show session path
  /compact       compact context
  /plan          turn plan mode on, off, or show status
  /approve       implement the pending plan
  /reject        discard the pending plan
  /clear         clear visible transcript
  /exit          quit

Model
  /model         show or switch model
  /effort        show or set effort
  /providers     list providers

Workspace
  /context       show loaded instructions
  /status        show git status
  /review        review current git diff
  /diff          show git diff
  /preview       serve a file locally
  /skills        list reusable workflow skills
  /subagent      run helper subagent

Keys
  Enter submit, Esc clear input or exit when empty, Ctrl+C exit, arrows move/scroll"#
}

fn run_ccx_command(workspace: &PathBuf, args: &[&str]) -> String {
    let Ok(exe) = std::env::current_exe() else {
        return "current executable unavailable".to_string();
    };
    let Ok(output) = Command::new(exe).args(args).current_dir(workspace).output() else {
        return format!("failed to run ccx {}", args.join(" "));
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let mut combined = String::new();
    if !stdout.trim().is_empty() {
        combined.push_str(stdout.trim_end());
    }
    if !stderr.trim().is_empty() {
        if !combined.is_empty() {
            combined.push('\n');
        }
        combined.push_str(stderr.trim_end());
    }
    if combined.is_empty() {
        format!("ccx {} exited {}", args.join(" "), output.status)
    } else {
        combined
    }
}

fn render_footer(state: &AgentState) {
    let session_short = short_session_id(state);
    let branch = git_branch(&state.context.workspace);
    let repo_state = git_repo_state(&state.context.workspace);
    ui::render_footer(FooterInfo {
        provider: &state.selected_provider,
        model: &state.selected_model,
        effort: state.current_effort().as_str(),
        permissions: &state.config.permission_profile,
        mode: state.work_mode.as_str(),
        branch: &branch,
        repo_state: &repo_state,
        session_short: &session_short,
    });
}

fn short_session_id(state: &AgentState) -> String {
    state.session.id.to_string().chars().take(8).collect()
}

fn parse_work_mode(value: &str) -> WorkMode {
    match value.trim().to_ascii_lowercase().as_str() {
        "plan" | "planning" => WorkMode::Plan,
        _ => WorkMode::Agent,
    }
}

fn git_branch(workspace: &PathBuf) -> String {
    let branch = run_git(workspace, &["rev-parse", "--abbrev-ref", "HEAD"]);
    if branch.starts_with("fatal:") || branch.trim().is_empty() {
        return "no-git".to_string();
    }
    branch.lines().next().unwrap_or("no-git").to_string()
}

fn git_repo_state(workspace: &PathBuf) -> String {
    let status = run_git(workspace, &["status", "--short"]);
    if status.starts_with("fatal:") {
        return "no-git".to_string();
    }
    if status.trim().is_empty() {
        "clean".to_string()
    } else {
        "dirty".to_string()
    }
}

fn git_status(workspace: &PathBuf) -> String {
    let status = run_git(workspace, &["status", "--short", "--branch"]);
    if status.trim().is_empty() {
        "clean".to_string()
    } else {
        status
    }
}

fn git_diff(workspace: &PathBuf) -> String {
    let mut output = run_git(workspace, &["diff", "--stat"]);
    let diff = run_git(workspace, &["diff", "--"]);
    if !output.trim().is_empty() && !diff.trim().is_empty() {
        output.push('\n');
    }
    output.push_str(&diff);
    output
}

fn run_git(workspace: &PathBuf, args: &[&str]) -> String {
    let Ok(output) = Command::new("git")
        .args(args)
        .current_dir(workspace)
        .output()
    else {
        return "git unavailable".to_string();
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if stdout.trim().is_empty() {
        stderr.trim().to_string()
    } else {
        stdout.trim_end().to_string()
    }
}

fn render_tool_results(results: &[ToolResult]) -> Result<String> {
    Ok(format!(
        "Tool results:\n{}",
        serde_json::to_string_pretty(results)?
    ))
}

async fn execute_tool_calls(
    state: &mut AgentState,
    tools: &ToolRegistry,
    user_input: &str,
    mut calls: Vec<ToolCall>,
    render_ui: bool,
    turn_mode: TurnMode,
) -> Result<Vec<ToolResult>> {
    for call in &mut calls {
        normalize_local_tool_path(user_input, call);
        state.session.append("tool_call", &*call)?;
        if render_ui {
            ui::working_for_tool(call);
            ui::render_tool_call(call);
        }
    }

    if matches!(turn_mode, TurnMode::PlanOnly) {
        let mut results = Vec::new();
        let mut allowed_calls = Vec::new();
        for call in calls {
            if is_read_only_call(&call) {
                allowed_calls.push(call);
            } else {
                results.push(blocked_plan_tool_result(call));
            }
        }
        let allowed_results = if allowed_calls.len() > 1 {
            join_all(allowed_calls.into_iter().map(|call| tools.execute(call))).await
        } else {
            let mut tool_results = Vec::new();
            for call in allowed_calls {
                tool_results.push(tools.execute(call).await);
            }
            tool_results
        };
        results.extend(allowed_results);
        for result in &results {
            if render_ui {
                ui::render_tool_result(result);
            }
            state.session.append("tool_result", result)?;
        }
        return Ok(results);
    }

    let results = if calls.iter().all(is_read_only_call) && calls.len() > 1 {
        join_all(calls.into_iter().map(|call| tools.execute(call))).await
    } else {
        let mut results = Vec::new();
        for call in calls {
            results.push(tools.execute(call).await);
        }
        results
    };

    for result in &results {
        if render_ui {
            ui::render_tool_result(result);
        }
        state.session.append("tool_result", result)?;
    }
    Ok(results)
}

fn blocked_plan_tool_result(call: ToolCall) -> ToolResult {
    ToolResult {
        tool: call.tool,
        call_id: call.call_id,
        success: false,
        content: "blocked by plan mode: only read-only tools are allowed before `/approve`"
            .to_string(),
        full_content: None,
        truncated: false,
    }
}

fn is_read_only_call(call: &ToolCall) -> bool {
    matches!(
        call.tool.as_str(),
        "read_file" | "glob" | "grep" | "git_status" | "git_diff"
    )
}

fn has_complete_streamed_tool_call(text: &str) -> bool {
    text.contains("</tool_call>")
}

fn auto_effort_for_task(configured: EffortLevel, user_input: &str) -> EffortLevel {
    let lowered = user_input.to_ascii_lowercase();
    if matches!(configured, EffortLevel::High | EffortLevel::Max) {
        return configured;
    }
    if lowered.contains("review")
        || lowered.contains("plan")
        || lowered.contains("search")
        || lowered.contains("find")
        || lowered.contains("status")
    {
        return EffortLevel::Low;
    }
    if lowered.contains("multi-file")
        || lowered.contains("architecture")
        || lowered.contains("refactor")
        || lowered.contains("debug")
    {
        return EffortLevel::High;
    }
    configured
}

fn routed_provider_model(state: &AgentState, user_input: &str) -> (String, String) {
    let lowered = user_input.to_ascii_lowercase();
    let simple = lowered.contains("review")
        || lowered.contains("search")
        || lowered.contains("find")
        || lowered.contains("status")
        || lowered.contains("list");
    if simple {
        if let Some((model, profile)) = state
            .config
            .model_profiles
            .iter()
            .find(|(_, profile)| profile.provider.as_deref() == Some("ollama"))
        {
            return (
                profile
                    .provider
                    .clone()
                    .unwrap_or_else(|| "ollama".to_string()),
                model.clone(),
            );
        }
    }
    (
        state.selected_provider.clone(),
        state.selected_model.clone(),
    )
}

fn selected_file_context(workspace: &PathBuf, user_input: &str) -> Option<String> {
    let mut files = Vec::new();
    for token in user_input.split_whitespace() {
        let cleaned = token.trim_matches(|ch: char| {
            ch == '"' || ch == '\'' || ch == '`' || ch == ',' || ch == '.' || ch == ':' || ch == ';'
        });
        if cleaned.contains('.') || cleaned.contains('/') || cleaned.contains('\\') {
            let path = workspace.join(cleaned);
            if path.is_file() {
                files.push(cleaned.replace('\\', "/"));
            }
        }
    }

    for line in git_diff(workspace).lines() {
        if let Some(path) = line.strip_prefix("+++ b/") {
            files.push(path.to_string());
        }
    }

    files.sort();
    files.dedup();
    files.truncate(8);
    if files.is_empty() {
        return None;
    }
    Some(format!(
        "Likely relevant files for this turn:\n{}",
        files.join("\n")
    ))
}

fn budget_messages(messages: Vec<ModelMessage>, budget: Option<usize>) -> Vec<ModelMessage> {
    let Some(budget) = budget else {
        return messages;
    };
    let mut total = 0usize;
    let mut kept = Vec::new();
    for message in messages.into_iter().rev() {
        let len = message.content.len();
        if total + len > budget && !matches!(message.role, MessageRole::System) {
            continue;
        }
        total += len;
        kept.push(message);
        if total >= budget {
            break;
        }
    }
    kept.reverse();
    kept
}

fn compact_transcript(transcript: &mut Vec<ModelMessage>) -> String {
    let keep = 8;
    if transcript.len() <= keep {
        return "context already compact".to_string();
    }

    let compacted_count = transcript.len().saturating_sub(keep);
    let mut files = Vec::new();
    let mut tool_success = 0;
    let mut tool_failures = 0;
    let mut user_goals = Vec::new();

    for message in transcript.iter().take(compacted_count) {
        match message.role {
            MessageRole::User => user_goals.push(truncate_for_summary(&message.content, 140)),
            MessageRole::Tool => {
                if message.content.contains("\"success\": true") {
                    tool_success += 1;
                }
                if message.content.contains("\"success\": false") {
                    tool_failures += 1;
                }
                for marker in ["wrote ", "edited ", "patched "] {
                    if let Some(index) = message.content.find(marker) {
                        let path = message.content[index + marker.len()..]
                            .lines()
                            .next()
                            .unwrap_or_default()
                            .trim();
                        if !path.is_empty() {
                            files.push(path.to_string());
                        }
                    }
                }
            }
            _ => {}
        }
    }

    files.sort();
    files.dedup();
    let summary = format!(
        "Compacted {compacted_count} messages. Goals: {}. Tool results: {tool_success} ok, {tool_failures} failed. Files touched: {}.",
        if user_goals.is_empty() {
            "none recorded".to_string()
        } else {
            user_goals.join(" | ")
        },
        if files.is_empty() {
            "none".to_string()
        } else {
            files.join(", ")
        }
    );

    let recent = transcript.split_off(compacted_count);
    transcript.clear();
    transcript.push(ModelMessage {
        role: MessageRole::System,
        content: format!("Session summary so far:\n{summary}"),
    });
    transcript.extend(recent);
    summary
}

fn truncate_for_summary(value: &str, max_chars: usize) -> String {
    let mut chars = value.chars();
    let truncated = chars.by_ref().take(max_chars).collect::<String>();
    if chars.next().is_some() {
        format!("{truncated}...")
    } else {
        truncated
    }
}

fn local_model_prompt(workspace: &str, instructions: &str, max_chars: Option<usize>) -> String {
    let mut prompt = format!(
        r#"You are ClaudeCodeX. Keep responses short.

Workspace: {workspace}

Use exactly one JSON action when you need a tool:
{{"action":"write_file","path":"file","content":"text"}}
{{"action":"read_file","path":"file"}}
{{"action":"read_file","path":"file","start_line":1,"line_count":80}}
{{"action":"shell","command":"cmd"}}

Return final text only when done. Keep output short.

Project instructions:
{instructions}"#
    );
    if let Some(max_chars) = max_chars {
        if prompt.len() > max_chars {
            prompt.truncate(max_chars);
            prompt.push_str("\n...[truncated]");
        }
    }
    prompt
}

fn plan_mode_prompt() -> &'static str {
    r#"PLAN MODE is active.

Create a concrete implementation plan only. Do not implement the task.
You may inspect the workspace with read-only tools: read_file, glob, grep, git_status, git_diff.
Do not call write_file, edit_file, shell, or any mutating tool.
The plan should include scope, files likely to change, implementation steps, risks, and verification.
End with a short approval instruction telling the user to run /approve to implement or /reject to discard."#
}

fn infer_file_write(user_input: &str, response: &str) -> Option<ToolCall> {
    let path = infer_target_path(user_input)?;
    let content = clean_file_content(response);
    if !looks_like_file_content(&content, &path) {
        return None;
    }
    Some(ToolCall {
        tool: "write_file".to_string(),
        arguments: json!({
            "path": path,
            "content": content
        }),
        call_id: None,
    })
}

fn normalize_local_tool_path(user_input: &str, call: &mut ToolCall) {
    if call.tool != "write_file" {
        return;
    }
    let Some(current) = call.arguments.get("path").and_then(|value| value.as_str()) else {
        return;
    };
    if !matches!(current, "file" | "output" | "untitled" | "page") {
        return;
    }
    if let Some(target) = infer_target_path(user_input) {
        call.arguments["path"] = json!(target);
    }
}

fn infer_target_path(user_input: &str) -> Option<String> {
    let lowered = user_input.to_ascii_lowercase();
    if !(lowered.contains("create")
        || lowered.contains("write")
        || lowered.contains("generate")
        || lowered.contains("make"))
    {
        return None;
    }

    for token in user_input.split_whitespace() {
        let cleaned = token.trim_matches(|ch: char| {
            ch == '"' || ch == '\'' || ch == '`' || ch == ',' || ch == '.' || ch == ':'
        });
        if cleaned.ends_with(".html")
            || cleaned.ends_with(".css")
            || cleaned.ends_with(".js")
            || cleaned.ends_with(".json")
            || cleaned.ends_with(".md")
        {
            return Some(cleaned.replace('\\', "/"));
        }
    }

    if lowered.contains("web page") || lowered.contains("html") {
        return Some("index.html".to_string());
    }
    None
}

fn clean_file_content(response: &str) -> String {
    let trimmed = response.trim();
    if let Some(block) = first_fenced_block(trimmed) {
        return block;
    }
    if let Some(stripped) = strip_fence(trimmed, "html") {
        return stripped;
    }
    if let Some(stripped) = strip_fence(trimmed, "") {
        return stripped;
    }
    trimmed.to_string()
}

fn first_fenced_block(text: &str) -> Option<String> {
    let start = text.find("```")?;
    let after_start = &text[start + 3..];
    let body_start = after_start
        .find('\n')
        .map(|index| start + 3 + index + 1)
        .unwrap_or(start + 3);
    let end = text[body_start..].find("```")? + body_start;
    Some(text[body_start..end].trim().to_string())
}

fn strip_fence(text: &str, language: &str) -> Option<String> {
    let start = if language.is_empty() {
        "```"
    } else {
        return text
            .strip_prefix(&format!("```{language}"))
            .and_then(|rest| rest.strip_suffix("```"))
            .map(|value| value.trim().to_string());
    };
    text.strip_prefix(start)
        .and_then(|rest| rest.strip_suffix("```"))
        .map(|value| value.trim().to_string())
}

fn looks_like_file_content(content: &str, path: &str) -> bool {
    let trimmed = content.trim_start();
    if path.ends_with(".html") {
        return trimmed.starts_with("<!DOCTYPE html")
            || trimmed.starts_with("<!doctype html")
            || trimmed.starts_with("<html");
    }
    if path.ends_with(".json") {
        return trimmed.starts_with('{') || trimmed.starts_with('[');
    }
    !trimmed.is_empty()
}

fn base_prompt() -> &'static str {
    r#"You are ClaudeCodeX, a terminal-only coding agent harness.

Your job is to solve software tasks in the current workspace. Be direct, inspect before editing, use tools when needed, and verify changes with focused commands when possible.

You must respect the harness permission profile. Do not claim that a tool ran unless a tool result says it ran. Keep final answers concise and grounded in observed results.

This is not a private provider prompt clone. Use your native strengths, but follow this visible tool protocol and complete the user's coding task end to end."#
}

#[derive(Serialize)]
struct EventText<T: Serialize> {
    text: T,
}

#[derive(Serialize)]
struct PlanEvent<'a> {
    task: &'a str,
    plan: &'a str,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_multiple_tool_calls() {
        let parsed = parse_model_output(
            r#"hi <tool_call>{"tool":"read_file","arguments":{"path":"Cargo.toml"}}</tool_call>
            <tool_call>{"tool":"git_status","arguments":{}}</tool_call>"#,
        )
        .unwrap();
        let calls = parsed.calls;
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].tool, "read_file");
        assert_eq!(calls[1].tool, "git_status");
    }

    #[test]
    fn strips_tool_call_blocks() {
        let parsed = parse_model_output(
            r#"before <tool_call>{"tool":"git_status","arguments":{}}</tool_call> after"#,
        )
        .unwrap();
        assert_eq!(parsed.visible_text.trim(), "before  after");
    }

    #[test]
    fn infers_html_write_for_web_page() {
        let call = infer_file_write(
            "Create a simple web page",
            "```html\n<!DOCTYPE html><html><body></body></html>\n```",
        )
        .unwrap();
        assert_eq!(call.tool, "write_file");
        assert_eq!(call.arguments["path"], "index.html");
    }

    #[test]
    fn infers_first_html_fence_before_explanation() {
        let call = infer_file_write(
            "Create index.html",
            "```html\n<!DOCTYPE html><html><body>ok</body></html>\n```\n\nI created it.",
        )
        .unwrap();
        assert_eq!(
            call.arguments["content"],
            "<!DOCTYPE html><html><body>ok</body></html>"
        );
    }

    #[test]
    fn normalizes_vague_local_file_path() {
        let mut call = ToolCall {
            tool: "write_file".to_string(),
            arguments: json!({"path":"file","content":"x"}),
            call_id: None,
        };
        normalize_local_tool_path("Create index.html", &mut call);
        assert_eq!(call.arguments["path"], "index.html");
    }
}

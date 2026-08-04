use std::io::{self, Read};

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use opticcode_policy::{AuditQuery, PolicyDecision, PolicyEngine, PolicyRequest};

const MAX_POLICY_INPUT_BYTES: u64 = 1024 * 1024;
pub const POLICY_EXIT_ALLOW: i32 = 0;
pub const POLICY_EXIT_REQUIRE_APPROVAL: i32 = 10;
pub const POLICY_EXIT_DENY: i32 = 11;

#[derive(Debug, Parser)]
#[command(name = "opticcode policy")]
#[command(about = "Inspect the deny-by-default OpticCode action policy.")]
pub struct PolicyArgs {
    #[command(subcommand)]
    command: PolicyCommand,
}

#[derive(Debug, Subcommand)]
enum PolicyCommand {
    /// Evaluate and audit one structured policy request read from stdin.
    Check {
        #[arg(long)]
        json: bool,
    },
    /// Explain one structured policy request from stdin without consuming approval or auditing.
    Explain {
        #[arg(long)]
        json: bool,
    },
    /// Read bounded, content-free policy audit records.
    Audit {
        #[arg(long, default_value_t = 100)]
        limit: usize,
        #[arg(long)]
        workspace_hash: Option<String>,
        #[arg(long)]
        action_kind: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

pub fn parse_isolated_policy() -> Option<PolicyArgs> {
    let mut args = std::env::args_os();
    args.next()?;
    let first = args.next()?;
    let mut parser_args = vec![std::ffi::OsString::from("opticcode policy")];
    if first == std::ffi::OsStr::new("policy") {
        parser_args.extend(args);
    } else if first == std::ffi::OsStr::new("help")
        && args.next().as_deref() == Some(std::ffi::OsStr::new("policy"))
    {
        parser_args.push(std::ffi::OsString::from("--help"));
        parser_args.extend(args);
    } else {
        return None;
    }
    Some(PolicyArgs::parse_from(parser_args))
}

pub fn run_policy(args: PolicyArgs) -> Result<i32> {
    let engine = PolicyEngine::default_engine()
        .map_err(|error| anyhow::anyhow!("failed to open policy runtime: {error}"))?;
    match args.command {
        PolicyCommand::Check { json } => {
            let request = read_request()?;
            let preflight = engine
                .check(&request)
                .map_err(|error| anyhow::anyhow!("policy check failed: {error}"))?;
            print_report(&preflight.report, json)?;
            return Ok(match preflight.report.decision {
                PolicyDecision::Allow { .. } => POLICY_EXIT_ALLOW,
                PolicyDecision::RequireApproval { .. } => POLICY_EXIT_REQUIRE_APPROVAL,
                PolicyDecision::Deny { .. } => POLICY_EXIT_DENY,
            });
        }
        PolicyCommand::Explain { json } => {
            let request = read_request()?;
            let preflight = engine
                .explain(&request)
                .map_err(|error| anyhow::anyhow!("policy explanation failed: {error}"))?;
            print_report(&preflight.report, json)?;
        }
        PolicyCommand::Audit {
            limit,
            workspace_hash,
            action_kind,
            json,
        } => {
            let report = engine.audit_store().list(&AuditQuery {
                limit,
                workspace_hash,
                action_kind,
            })?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!("Policy audit: {} event(s)", report.events.len());
                println!("- storage: {}", report.storage.display());
                println!(
                    "- ignored incomplete records: {}",
                    report.ignored_partial_records
                );
                for event in &report.events {
                    println!(
                        "- {} {} {} rule={} result={}",
                        event.timestamp_unix_ms,
                        event.action_kind,
                        event.decision,
                        event.rule_id,
                        event.result
                    );
                }
            }
        }
    }
    Ok(POLICY_EXIT_ALLOW)
}

fn read_request() -> Result<PolicyRequest> {
    let mut bytes = Vec::new();
    io::stdin()
        .lock()
        .take(MAX_POLICY_INPUT_BYTES + 1)
        .read_to_end(&mut bytes)
        .context("failed to read policy request from stdin")?;
    if bytes.is_empty() {
        bail!("policy request stdin is empty");
    }
    if bytes.len() as u64 > MAX_POLICY_INPUT_BYTES {
        bail!("policy request exceeds the {MAX_POLICY_INPUT_BYTES}-byte bound");
    }
    serde_json::from_slice(&bytes).context("policy request is not one valid JSON object")
}

fn print_report(report: &opticcode_policy::PolicyReport, json: bool) -> Result<()> {
    if json {
        println!("{}", serde_json::to_string_pretty(report)?);
        return Ok(());
    }
    println!("Policy decision: {}", report.decision.kind());
    println!("- rule: {}", report.decision.rule_id());
    println!("- action: {}", report.action_kind);
    println!("- reason: {}", report.user_reason);
    println!("- recommendation: {}", report.recommended_action);
    println!("- revalidation required: {}", report.revalidation_required);
    if let PolicyDecision::RequireApproval { risk, .. } | PolicyDecision::Deny { risk, .. } =
        &report.decision
    {
        println!("- risk: {risk:?}");
    }
    if let Some(event_id) = &report.audit_event_id {
        println!("- audit event: {event_id}");
    }
    Ok(())
}

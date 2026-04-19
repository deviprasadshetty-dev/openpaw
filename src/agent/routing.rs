use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChatType {
    Direct,
    Group,
    Channel,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
pub struct PeerRef {
    pub kind: ChatType,
    pub id: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct BindingMatch {
    pub channel: Option<String>,
    pub account_id: Option<String>,
    pub peer: Option<PeerRef>,
    pub guild_id: Option<String>,
    pub team_id: Option<String>,
    #[serde(default)]
    pub roles: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AgentBinding {
    pub agent_id: String,
    pub comment: Option<String>,
    #[serde(default)]
    pub match_: BindingMatch,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MatchedBy {
    Peer,
    ParentPeer,
    GuildRoles,
    Guild,
    Team,
    Account,
    ChannelOnly,
    Default,
}

#[derive(Debug, Clone)]
pub struct ResolvedRoute {
    pub agent_id: String,
    pub channel: String,
    pub account_id: String,
    pub session_key: String,
    pub main_session_key: String,
    pub matched_by: MatchedBy,
}

#[derive(Debug, Clone, Default)]
pub struct RouteInput {
    pub channel: String,
    pub account_id: String,
    pub peer: Option<PeerRef>,
    pub parent_peer: Option<PeerRef>,
    pub guild_id: Option<String>,
    pub team_id: Option<String>,
    pub member_role_ids: Vec<String>,
}

pub fn normalize_id(input: &str) -> String {
    if input.is_empty() {
        return "default".to_string();
    }

    let mut normalized = String::with_capacity(input.len());
    for c in input.chars() {
        if normalized.len() >= 64 {
            break;
        }
        if c.is_alphanumeric() {
            normalized.push(c.to_ascii_lowercase());
        } else {
            normalized.push('-');
        }
    }

    let trimmed = normalized.trim_matches('-');
    if trimmed.is_empty() {
        "default".to_string()
    } else {
        trimmed.to_string()
    }
}

pub fn build_session_key(agent_id: &str, channel: &str, peer: Option<&PeerRef>) -> String {
    let norm_agent = normalize_id(agent_id);

    if let Some(p) = peer {
        let kind_str = match p.kind {
            ChatType::Direct => "direct",
            ChatType::Group => "group",
            ChatType::Channel => "channel",
        };

        format!("agent:{}:{}:{}:{}", norm_agent, channel, kind_str, p.id)
    } else {
        format!("agent:{}:{}:none:none", norm_agent, channel)
    }
}

pub fn build_main_session_key(agent_id: &str) -> String {
    format!("agent:{}:main", normalize_id(agent_id))
}

pub fn build_thread_session_key(base_key: &str, thread_id: &str) -> String {
    format!("{}:thread:{}", base_key, thread_id)
}

pub fn resolve_thread_parent_session_key(key: &str) -> Option<&str> {
    key.rfind(":thread:").map(|idx| &key[0..idx])
}



fn binding_matches_scope(binding: &AgentBinding, input: &RouteInput) -> bool {
    if let Some(ref bc) = binding.match_.channel
        && bc != &input.channel {
            return false;
        }
    if let Some(ref ba) = binding.match_.account_id
        && ba != &input.account_id {
            return false;
        }
    true
}

fn is_account_only(b: &AgentBinding) -> bool {
    b.match_.peer.is_none()
        && b.match_.guild_id.is_none()
        && b.match_.team_id.is_none()
        && b.match_.roles.is_empty()
}

fn is_channel_only(b: &AgentBinding) -> bool {
    b.match_.account_id.is_none()
        && b.match_.peer.is_none()
        && b.match_.guild_id.is_none()
        && b.match_.team_id.is_none()
        && b.match_.roles.is_empty()
}

fn has_matching_role(binding_roles: &[String], member_roles: &[String]) -> bool {
    binding_roles.iter().any(|br| member_roles.contains(br))
}

fn all_constraints_match(
    b: &AgentBinding,
    input: &RouteInput,
    check_peer: Option<&PeerRef>,
) -> bool {
    if let Some(ref bp) = b.match_.peer {
        if let Some(ip) = check_peer {
            if bp.kind != ip.kind || bp.id != ip.id {
                return false;
            }
        } else {
            return false;
        }
    }

    if let Some(ref bg) = b.match_.guild_id
        && Some(bg) != input.guild_id.as_ref() {
            return false;
        }

    if let Some(ref bt) = b.match_.team_id
        && Some(bt) != input.team_id.as_ref() {
            return false;
        }

    if !b.match_.roles.is_empty()
        && !has_matching_role(&b.match_.roles, &input.member_role_ids) {
            return false;
        }

    true
}

fn build_route(agent_id: &str, input: &RouteInput, matched_by: MatchedBy) -> ResolvedRoute {
    ResolvedRoute {
        agent_id: agent_id.to_string(),
        channel: input.channel.clone(),
        account_id: input.account_id.clone(),
        session_key: build_session_key(agent_id, &input.channel, input.peer.as_ref()),
        main_session_key: build_main_session_key(agent_id),
        matched_by,
    }
}

pub fn resolve_route(
    input: &RouteInput,
    bindings: &[AgentBinding],
    default_agent: &str,
) -> ResolvedRoute {
    let candidates: Vec<&AgentBinding> = bindings
        .iter()
        .filter(|b| binding_matches_scope(b, input))
        .collect();

    // Tier 1: peer match
    if let Some(ref ip) = input.peer {
        for b in &candidates {
            if b.match_.peer.is_some() && all_constraints_match(b, input, Some(ip)) {
                return build_route(&b.agent_id, input, MatchedBy::Peer);
            }
        }
    }

    // Tier 2: parent_peer match
    if let Some(ref pp) = input.parent_peer
        && !pp.id.is_empty() {
            for b in &candidates {
                if b.match_.peer.is_some() && all_constraints_match(b, input, Some(pp)) {
                    return build_route(&b.agent_id, input, MatchedBy::ParentPeer);
                }
            }
        }

    // Tier 3: guild_id + roles match
    if input.guild_id.is_some() && !input.member_role_ids.is_empty() {
        for b in &candidates {
            if b.match_.guild_id.is_some()
                && !b.match_.roles.is_empty()
                && all_constraints_match(b, input, input.peer.as_ref())
            {
                return build_route(&b.agent_id, input, MatchedBy::GuildRoles);
            }
        }
    }

    // Tier 4: guild_id only
    if input.guild_id.is_some() {
        for b in &candidates {
            if b.match_.guild_id.is_some()
                && b.match_.roles.is_empty()
                && all_constraints_match(b, input, input.peer.as_ref())
            {
                return build_route(&b.agent_id, input, MatchedBy::Guild);
            }
        }
    }

    // Tier 5: team_id match
    if input.team_id.is_some() {
        for b in &candidates {
            if b.match_.team_id.is_some() && all_constraints_match(b, input, input.peer.as_ref()) {
                return build_route(&b.agent_id, input, MatchedBy::Team);
            }
        }
    }

    // Tier 6: account only
    for b in &candidates {
        if b.match_.account_id.is_some() && is_account_only(b) {
            return build_route(&b.agent_id, input, MatchedBy::Account);
        }
    }

    // Tier 7: channel only
    for b in &candidates {
        if is_channel_only(b) {
            return build_route(&b.agent_id, input, MatchedBy::ChannelOnly);
        }
    }

    build_route(default_agent, input, MatchedBy::Default)
}

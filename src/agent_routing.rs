use crate::config_types::{
    AgentBinding, ChatType, DmScope, IdentityLink, MatchedBy, NamedAgentConfig, PeerRef,
    ResolvedRoute, RouteInput, SessionConfig,
};

// ═══════════════════════════════════════════════════════════════════════════
// Helpers
// ═══════════════════════════════════════════════════════════════════════════

/// Normalize an ID: lowercase, replace non-alphanumeric with '-',
/// strip leading/trailing dashes, cap at 64 chars.
/// Returns "default" for empty or all-dash input.
pub fn normalize_id(input: &str) -> String {
    if input.is_empty() {
        return "default".to_string();
    }

    let mut buf = String::with_capacity(64);
    let mut len = 0;

    for c in input.chars() {
        if len >= 64 {
            break;
        }
        if c.is_alphanumeric() {
            buf.push(c.to_ascii_lowercase());
            len += 1;
        } else {
            buf.push('-');
            len += 1;
        }
    }

    let trimmed = buf.trim_matches('-');
    if trimmed.is_empty() {
        "default".to_string()
    } else {
        trimmed.to_string()
    }
}

/// Resolve a peer ID through identity links. If the peer matches any
/// link's peers list, return the canonical name instead.
pub fn resolve_linked_peer_id(peer_id: &str, identity_links: &[IdentityLink]) -> String {
    for link in identity_links {
        for linked_peer in &link.peers {
            if linked_peer == peer_id {
                return link.canonical.clone();
            }
        }
    }
    peer_id.to_string()
}

/// Build a DM-scope-aware session key.
pub fn build_session_key(agent_id: &str, channel: &str, peer: Option<&PeerRef>) -> String {
    build_session_key_with_scope(agent_id, channel, peer, DmScope::PerChannelPeer, None, &[])
}

/// Build a session key respecting DmScope and identity links.
pub fn build_session_key_with_scope(
    agent_id: &str,
    channel: &str,
    peer: Option<&PeerRef>,
    dm_scope: DmScope,
    account_id: Option<&str>,
    identity_links: &[IdentityLink],
) -> String {
    let norm_agent = normalize_id(agent_id);

    if let Some(p) = peer {
        let kind_str = match p.kind {
            ChatType::Direct => "direct",
            ChatType::Group => "group",
            ChatType::Channel => "channel",
        };

        // Groups and channels always use per-channel-peer scope
        if p.kind != ChatType::Direct {
            return format!("agent:{}:{}:{}:{}", norm_agent, channel, kind_str, p.id);
        }

        // Resolve identity links for DM peers
        let resolved_peer = resolve_linked_peer_id(&p.id, identity_links);

        match dm_scope {
            DmScope::Main => format!("agent:{}:main", norm_agent),
            DmScope::PerPeer => format!("agent:{}:direct:{}", norm_agent, resolved_peer),
            DmScope::PerChannelPeer => {
                format!("agent:{}:{}:direct:{}", norm_agent, channel, resolved_peer)
            }
            DmScope::PerAccountChannelPeer => format!(
                "agent:{}:{}:{}:direct:{}",
                norm_agent,
                channel,
                account_id.unwrap_or("default"),
                resolved_peer
            ),
        }
    } else {
        format!("agent:{}:{}:none:none", norm_agent, channel)
    }
}

/// Build the main session key for an agent: `agent:{id}:main`.
pub fn build_main_session_key(agent_id: &str) -> String {
    let norm_agent = normalize_id(agent_id);
    format!("agent:{}:main", norm_agent)
}

/// Append `:thread:{threadId}` to a base session key.
pub fn build_thread_session_key(base_key: &str, thread_id: &str) -> String {
    format!("{}:thread:{}", base_key, thread_id)
}

/// Strip `:thread:{threadId}` suffix to get the parent session key.
/// Returns null if the key doesn't contain a thread suffix.
pub fn resolve_thread_parent_session_key(key: &str) -> Option<String> {
    let marker = ":thread:";
    if let Some(idx) = key.rfind(marker) {
        return Some(key[0..idx].to_string());
    }
    None
}

/// Find the default agent from a named agents list.
/// Returns the first agent's name, or "main" if the list is empty.
pub fn find_default_agent(agents: &[NamedAgentConfig]) -> String {
    if let Some(agent) = agents.first() {
        agent.name.clone()
    } else {
        "main".to_string()
    }
}

/// Check if two PeerRef values match (same kind and id).
pub fn peer_matches(binding_peer: Option<&PeerRef>, input_peer: Option<&PeerRef>) -> bool {
    match (binding_peer, input_peer) {
        (Some(bp), Some(ip)) => bp.kind == ip.kind && bp.id == ip.id,
        _ => false,
    }
}

/// Pre-filter: check that a binding's channel and account_id constraints
/// match the input. A null constraint means "any" (matches everything).
pub fn binding_matches_scope(binding: &AgentBinding, input: &RouteInput) -> bool {
    if let Some(bc) = &binding.r#match.channel
        && bc != &input.channel {
            return false;
        }
    if let Some(ba) = &binding.r#match.account_id
        && ba != &input.account_id {
            return false;
        }
    true
}

/// Returns true if the binding has no peer, guild_id, team_id, or roles set
/// (only channel and/or account_id).
fn is_account_only(b: &AgentBinding) -> bool {
    b.r#match.peer.is_none()
        && b.r#match.guild_id.is_none()
        && b.r#match.team_id.is_none()
        && b.r#match.roles.is_empty()
}

/// Returns true if the binding has only a channel constraint (no account_id,
/// peer, guild_id, team_id, or roles).
fn is_channel_only(b: &AgentBinding) -> bool {
    b.r#match.account_id.is_none()
        && b.r#match.peer.is_none()
        && b.r#match.guild_id.is_none()
        && b.r#match.team_id.is_none()
        && b.r#match.roles.is_empty()
}

/// Check ALL constraints on a binding against input. Each constraint on the
/// binding must match the corresponding input field. Null constraints match
/// anything (they impose no restriction).
fn all_constraints_match(
    b: &AgentBinding,
    input: &RouteInput,
    check_peer: Option<&PeerRef>,
) -> bool {
    // Channel + account_id already checked in pre-filter.
    // Peer constraint: if binding has a peer, it must match the given check_peer.
    if let Some(bp) = &b.r#match.peer {
        if let Some(ip) = check_peer {
            if bp.kind != ip.kind || bp.id != ip.id {
                return false;
            }
        } else {
            return false;
        }
    }
    // Guild constraint
    if let Some(bg) = &b.r#match.guild_id {
        if let Some(ig) = &input.guild_id {
            if bg != ig {
                return false;
            }
        } else {
            return false;
        }
    }
    // Team constraint
    if let Some(bt) = &b.r#match.team_id {
        if let Some(it) = &input.team_id {
            if bt != it {
                return false;
            }
        } else {
            return false;
        }
    }
    // Roles constraint
    if !b.r#match.roles.is_empty()
        && !has_matching_role(&b.r#match.roles, &input.member_role_ids) {
            return false;
        }
    true
}

/// Check if any role in `binding_roles` appears in `member_roles`.
fn has_matching_role(binding_roles: &[String], member_roles: &[String]) -> bool {
    for br in binding_roles {
        for mr in member_roles {
            if br == mr {
                return true;
            }
        }
    }
    false
}

// ═══════════════════════════════════════════════════════════════════════════
// Route resolution
// ═══════════════════════════════════════════════════════════════════════════

/// Resolve the agent route for a given input.
///
/// Walks 7 tiers of binding matches in priority order and returns the
/// first match found. Falls back to the default agent if none match.
pub fn resolve_route(
    input: &RouteInput,
    bindings: &[AgentBinding],
    agents: &[NamedAgentConfig],
) -> ResolvedRoute {
    // For backward compatibility, use default SessionConfig.
    let default_session_config = SessionConfig {
        dm_scope: DmScope::PerChannelPeer,
        identity_links: vec![],
        idle_minutes: 60,
        typing_interval_secs: 5,
    };
    resolve_route_with_session(input, bindings, agents, &default_session_config)
}

/// Resolve the agent route for a given input using SessionConfig.
pub fn resolve_route_with_session(
    input: &RouteInput,
    bindings: &[AgentBinding],
    agents: &[NamedAgentConfig],
    session_config: &SessionConfig,
) -> ResolvedRoute {
    // Pre-filter bindings by channel + account_id scope.
    let scoped_bindings: Vec<&AgentBinding> = bindings
        .iter()
        .filter(|b| binding_matches_scope(b, input))
        .collect();

    let mut matched_agent = None;
    let mut matched_by = MatchedBy::Default;

    // 1. Peer Match (exact kind + id)
    if matched_agent.is_none()
        && let Some(input_peer) = &input.peer {
            for b in &scoped_bindings {
                if let Some(bp) = &b.r#match.peer
                    && peer_matches(Some(bp), Some(input_peer))
                        && all_constraints_match(b, input, Some(input_peer))
                    {
                        matched_agent = Some(b.agent_id.clone());
                        matched_by = MatchedBy::Peer;
                        break;
                    }
            }
        }

    // 2. Parent Peer Match
    if matched_agent.is_none()
        && let Some(parent_peer) = &input.parent_peer {
            for b in &scoped_bindings {
                if let Some(bp) = &b.r#match.peer
                    && peer_matches(Some(bp), Some(parent_peer))
                        && all_constraints_match(b, input, Some(parent_peer))
                    {
                        matched_agent = Some(b.agent_id.clone());
                        matched_by = MatchedBy::ParentPeer;
                        break;
                    }
            }
        }

    // 3. Guild Roles Match
    if matched_agent.is_none() && input.guild_id.is_some() && !input.member_role_ids.is_empty() {
        for b in &scoped_bindings {
            if !b.r#match.roles.is_empty() && all_constraints_match(b, input, input.peer.as_ref()) {
                matched_agent = Some(b.agent_id.clone());
                matched_by = MatchedBy::GuildRoles;
                break;
            }
        }
    }

    // 4. Guild Match
    if matched_agent.is_none()
        && let Some(guild_id) = &input.guild_id {
            for b in &scoped_bindings {
                // Must have guild_id, no peer, no team, no roles
                if b.r#match.guild_id.as_deref() == Some(guild_id)
                    && b.r#match.peer.is_none()
                    && b.r#match.team_id.is_none()
                    && b.r#match.roles.is_empty()
                {
                    matched_agent = Some(b.agent_id.clone());
                    matched_by = MatchedBy::Guild;
                    break;
                }
            }
        }

    // 5. Team Match
    if matched_agent.is_none()
        && let Some(team_id) = &input.team_id {
            for b in &scoped_bindings {
                // Must have team_id, no peer, no guild, no roles
                if b.r#match.team_id.as_deref() == Some(team_id)
                    && b.r#match.peer.is_none()
                    && b.r#match.guild_id.is_none()
                    && b.r#match.roles.is_empty()
                {
                    matched_agent = Some(b.agent_id.clone());
                    matched_by = MatchedBy::Team;
                    break;
                }
            }
        }

    // 6. Account Match
    if matched_agent.is_none() {
        for b in &scoped_bindings {
            if is_account_only(b) && b.r#match.account_id.is_some() {
                matched_agent = Some(b.agent_id.clone());
                matched_by = MatchedBy::Account;
                break;
            }
        }
    }

    // 7. Channel Only Match
    if matched_agent.is_none() {
        for b in &scoped_bindings {
            if is_channel_only(b) && b.r#match.channel.is_some() {
                matched_agent = Some(b.agent_id.clone());
                matched_by = MatchedBy::ChannelOnly;
                break;
            }
        }
    }

    // Default Fallback
    let final_agent_id = matched_agent.unwrap_or_else(|| find_default_agent(agents));

    // Construct keys
    let session_key = build_session_key_with_scope(
        &final_agent_id,
        &input.channel,
        input.peer.as_ref(),
        session_config.dm_scope,
        Some(&input.account_id),
        &session_config.identity_links,
    );
    let main_session_key = build_main_session_key(&final_agent_id);

    ResolvedRoute {
        agent_id: final_agent_id,
        channel: input.channel.clone(),
        account_id: input.account_id.clone(),
        session_key,
        main_session_key,
        matched_by,
    }
}

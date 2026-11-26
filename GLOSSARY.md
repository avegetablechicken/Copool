# Copool

Copool keeps the accounts of several AI subscriptions, shows their usage, and switches the matching coding tool between them.

## Language

### Pools and providers

**Pool**:
One subscription family whose accounts Copool keeps together and whose tool it switches: Codex, Claude, Grok or Antigravity.
_Avoid_: platform, vendor

**Sub2api provider**:
A Sub2api relay that a **Pool**'s tool can be switched to, with the administrator login Copool reads its accounts through.
_Avoid_: relay, site, upstream (for the provider itself)

**Gateway**:
The address a **Pool**'s tool sends its requests to at a **Sub2api provider**; distinct from the provider's admin address, which Copool alone uses.
_Avoid_: base URL (when the gateway is meant)

**Provider registry**:
The **Sub2api providers** Copool keeps for a **Pool**, with each one's saved form, password and accounts.
_Avoid_: provider list, provider settings

### Switching

**Env bridge**:
Copool's block in a tool's `.env` file, together with the line in the user's shell startup file that loads it, through which a switch to a **Sub2api provider** gives the tool its key and **Gateway**.
_Avoid_: env block, rc hook

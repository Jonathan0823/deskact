# DeskAct

This glossary defines the product language for DeskAct's user requests, model-produced plans, and configurable LLM endpoints.

## App selection language

**App catalog**: The discovered and explicitly configured app entries DeskAct may consider for an app request. Each entry is the local source of truth for its app identity and launch target.
_Avoid_: model-generated app list

**Catalog-grounded app selection**: The primary model's interpretation of natural-language app intent by choosing a supplied catalog entry or asking for clarification. The model does not create app identities, executable paths, or launch permission.
_Avoid_: AI-generated target

**User-defined app alias**: An optional, explicitly saved phrase associated with a catalog entry to support exact matching. It is a convenience, not a requirement for natural-language app selection, and is not learned automatically.
_Avoid_: learned alias

## Approval and risk language

**Consequential action**: An action that is destructive, externally visible, or materially changes system settings, and therefore requires explicit user approval. DeskAct classifies the action; model claims cannot lower its risk.
_Avoid_: model-approved action

**Action scope**: The exact operation, target, and parameters shown for a proposed action. Changing any part creates a different scope.
_Avoid_: approval context (when referring to the action itself)

**Action-specific approval**: The user's explicit permission for one action scope. It cannot authorize a different operation, target, or parameter set.
_Avoid_: blanket approval

## Task outcome language

**Verified outcome**: A task result that DeskAct confirmed from independent evidence, such as reading back the text or the saved file. Only verified outcomes count toward reliability gates.
_Avoid_: successful task (when no independent evidence exists)

**Model-reported outcome**: A task result that rests only on the primary model's own claim that it finished. It is shown to the user as unverified.
_Avoid_: completed, done (without qualification)

## LLM planning and API language

**Primary model**: The single user-configured LLM endpoint and model used to interpret unmatched requests, propose action plans, and propose tool calls in the agent loop.
_Avoid_: router model, secondary model

**Action plan**: A structured proposal describing candidate actions and expected outcomes for a user request. It is not authorization to execute those actions.
_Avoid_: executable command, tool call (when referring to the whole plan)

**Structured output**: A response format constrained by an endpoint or model to follow a declared structure or schema. Conformance to that format does not establish that the content is semantically correct or safe.
_Avoid_: validated plan

**Model-proposed tool call**: A model's request to invoke a named operation with arguments. It is a proposal, not execution or permission; DeskAct's policy remains authoritative.
_Avoid_: tool execution, authorization

**OpenAI-compatible endpoint**: An API endpoint that implements some OpenAI-shaped request and response conventions. The label alone does not imply support for every endpoint, parameter, or feature.
_Avoid_: fully OpenAI-compatible (unless the exact feature set is specified)

**Chat Completions interface**: The OpenAI-style chat request and response shape DeskAct requires from an MVP endpoint. It does not imply support for other OpenAI API surfaces or optional capabilities.
_Avoid_: OpenAI API compatibility (when referring to this specific interface)

**Agent loop**: The step-by-step exchange in which the primary model observes the target window, proposes tool calls, and receives the resulting observation, until the task finishes, needs the user, or exhausts its budget. Each proposed call stays subject to DeskAct policy.
_Avoid_: autonomous mode, plan execution (for this exchange)

**Target window**: The single accessible, non-minimized window of the selected app that DeskAct has verified as the one a task may act on, together with the dialogs and popups it owns in the same process. Observations and tool calls are confined to it; any other window, including another window of the same app, is out of scope.
_Avoid_: active window, focused app

**Model discovery**: An endpoint-provided listing of model identifiers available to a service. A listed identifier does not certify its capabilities.
_Avoid_: capability discovery

## Browser automation language

**Dedicated automation profile**: A browser profile kept separate from a user's everyday browsing for DeskAct automation. Its cookies, authentication, and site data are visible to the attached automation; minimum relevant page context may be sent to the configured LLM for a user-requested task. It is not a security-isolation boundary.
_Avoid_: everyday profile, isolated profile

**CDP-attached browser session**: A session in which DeskAct connects to an already-running browser through the Chrome DevTools Protocol. Tab pinning can constrain the target tab, but does not isolate the session's browser data.
_Avoid_: isolated browser session

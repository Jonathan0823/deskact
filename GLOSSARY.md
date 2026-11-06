# DeskAct

This glossary defines the product language for DeskAct's user requests, model-produced plans, and configurable LLM endpoints.

## App selection language

**App catalog**: The discovered and explicitly configured app entries DeskAct may consider for an app request. Each entry is the local source of truth for its app identity and launch target.
_Avoid_: model-generated app list

**Catalog-grounded app selection**: The primary model's interpretation of natural-language app intent by choosing a supplied catalog entry or asking for clarification. The model does not create app identities, executable paths, or launch permission.
_Avoid_: AI-generated target

**User-defined app alias**: An optional, explicitly saved phrase associated with a catalog entry to support exact matching. It is a convenience, not a requirement for natural-language app selection, and is not learned automatically.
_Avoid_: learned alias

## LLM planning and API language

**Primary model**: The single user-configured LLM endpoint and model used to interpret unmatched requests and propose action plans.
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

**Model discovery**: An endpoint-provided listing of model identifiers available to a service. A listed identifier does not certify its capabilities.
_Avoid_: capability discovery

## Browser automation language

**Dedicated automation profile**: A browser profile kept separate from a user's everyday browsing for DeskAct automation. Its cookies, authentication, and site data are visible to the attached automation; minimum relevant page context may be sent to the configured LLM for a user-requested task. It is not a security-isolation boundary.
_Avoid_: everyday profile, isolated profile

**CDP-attached browser session**: A session in which DeskAct connects to an already-running browser through the Chrome DevTools Protocol. Tab pinning can constrain the target tab, but does not isolate the session's browser data.
_Avoid_: isolated browser session

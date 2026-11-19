# mygitui

Fast, configurable Git GUI with AI-assisted workflows.

## Language

### AI

**AI feature**:
A discrete, user-invocable AI capability declared in the feature registry (e.g. commit message, explain hunk, diagnose workflow).
_Avoid_: AI action, AI tool

**Single-shot run**:
An AI feature execution where all context is gathered up front and sent as one prompt; the model cannot request more data.
_Avoid_: quick run, simple mode

**Agentic run**:
An AI feature execution where the model may invoke repo tools to gather context or act, with tool approvals surfaced to the user.
_Avoid_: autonomous mode, agent run

**Repo tool**:
A discrete capability an agentic run may invoke — read a file, search, run a command, mutate git state.
_Avoid_: function, skill

**Tool approval**:
A user decision resolving a pending repo-tool invocation: allow once, allow always in this repo, or deny.
_Avoid_: permission grant

**Tool policy**:
The rule set an agentic run is launched with, declaring which repo tools are auto-allowed, which require approval, and which are denied outright.
_Avoid_: permission config, guardrails

**Feature mode**:
The per-feature execution preference: auto (agentic when available, otherwise single-shot), single-shot, or off.
_Avoid_: agentic toggle

**Context chip**:
An optional, user-toggled piece of repository context attached to a chat message.
_Avoid_: context block

**Backend**:
One of the two AI services features run against: opencode or OpenRouter.
_Avoid_: provider (overloaded — means the LLM provider inside a backend)

### UI

**Agent mode**:
A workspace layout (changes / diff / PR columns) that hosts the assistant chat. It is a layout, not an AI capability — nothing about it is agentic.
_Avoid_: calling any AI capability "agent mode"

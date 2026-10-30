# Changelog

Every Larrier release and what changed in it. Versions follow
[Semantic Versioning](https://semver.org): the major number for breaking
changes, the minor for new features, the patch for fixes. Previews carry a
suffix such as `-beta.2`.

## 0.1.11 — 2026-10-30

### New

- OpenCode and Hermes connect over ACP, so they can ask
## 0.1.10 — 2026-10-30

### New

- Codex runs through its app server, asking live

### Fixed

- A request card says what's being asked
- A Codex plan is built with the permissions picked
- Mode and permission chips sit side by side
## 0.1.9 — 2026-10-30

### New

- MCP servers a Claude agent defines for itself
- Run a Claude Code chat as one of the user's agents
- Pick the agent a new chat runs as from @

### Look and feel

- A reply starts where the prompt's text does

### Fixed

- The @ menu stays in view and opens down when it fits only there
- The gliding cursor glides only along a line
- Opened projects stay open across launches
- Wait for Claude Code's MCP servers before listing them
## 0.1.8 — 2026-10-30

### New

- Read Claude Code's commands, subagents and MCP servers
- Show the MCP servers agents report themselves
- The agent's own / commands, @ subagents, /mcp and /usage
## 0.1.7 — 2026-10-30

### New

- A typing cursor that glides
## 0.1.6 — 2026-10-30

### New

- An update button that's only there when there's an update
## 0.1.5 — 2026-10-30

### Fixed

- Prompts keep their images and files when a chat re-syncs
- Steps start at the reply's left edge, like its text and cards
- Copy on code blocks copies
## 0.1.4 — 2026-10-30

### New

- A steered message shows at once, greyed until it's read

### Fixed

- Menus stay inside the window when their contents grow
- Ctrl+Backspace deletes a word again in text fields
## 0.1.3 — 2026-10-30

### New

- Turn agents on and off, and a clearer agent card
## 0.1.2 — 2026-10-30

### New

- Answer a running agent live, steer it, and queue messages like Cursor
- Say what an agent refused because it couldn't ask
- Pane toggles that stay put, and panes that move smoothly
- Larrier's own tooltips instead of the system's
- Chats with new activity come first
- The mark's stem becomes a dry brush stroke

### Look and feel

- Drop the focus ring and glow

### Fixed

- Release script pushes with the GitHub CLI's login
- Give imported chats their folder and resume them where they live
- Claude Code's own model menu, and the pick stays checked
- Edit a sent prompt in place, keeping its images and files
- Rows of a turn line up, terminals are background commands, previews hold
## 0.1.1 — 2026-10-30

### New

- Semantic versions, update channels and release notes
## 0.1.0 — 2026-10-30

The first release.

### New

- Track subagents and background tasks end to end
- Appearance, behavior and sidebar settings
- Tabs, split view, drag & drop, fast transcripts and settings revamp
- Model registry with names, context windows and prices
- Sessions from other tools update live in Larrier
- Menus stay inside the window; Cursor-style repo rows
- Show models by name with context and price; registry browser
- Read turn state, queued prompts and images from transcripts
- A session running in its own app looks like any running session
- Render every tool properly, MCP included
- @-mentions and /-skills are real chips
- Smooth streaming motion (off with Reduce Motion)
- A prompt timeline that opens on hover
- Each agent's real modes and permission settings
- Import every Claude Code record type properly
- Handle every Claude Code stream event, checked against real runs
- Label task, cron and team tools
- Smooth running spinner
- Fixed sidebar order and unread marks
- Cursor-style tool step wording
- Import Claude Code mode, pull requests and relocation
- Pull request link in the chat header
- Show images returned by commands
- Import images in Gemini CLI chats
- One session per window, with its subagents as tabs
- Side panel tabs that keep or preview, and background tasks
- Collapsible session rail with a session summary
- Live Claude plan limits in the composer chip
- Re-import sessions when an importer improves
- Drop a chat where the pointer is, no options panel
- Record how full the model's context is
- Session rail with context, to-dos, activity and changes
- Mark tool calls that never ran
- The stencil logo, on ink or paper
- Cursor-style prompt timeline cards
- A text size for each kind of text, set with − / +
- Prompt cards with thumbnails on top and a fixed height
- Double-click a resizer to fit the window
- Telescope the timeline under the pointer
- Answer an agent's questions in the chat, for every agent
- Show tools live while the agent writes them
- Show what Claude Code subagents say, and what they run on
- Background work as Cursor-style chips, cards and panel
- The agent's terminals, read-only in the Terminal tab
- Edit an earlier prompt in place
- Mark every session as read at once
- Choose the corners of buttons — Solid, Balanced, Rounded
- Projects and Home in the sidebar, a Mixed grouping, and motion
- Edit cards are colored by the file's language
- Install, update and uninstall like a regular Windows app
- Read each agent's own models, defaults and providers
- Reasoning effort and your own endpoints at launch
- Model management per agent, synced with each agent

### Look and feel

- Order the model registry and keep names readable
- Pinned edit card, softer shadows, fade above the composer
- No block caret after streaming text
- A smaller, quieter background-tasks card
- Codex wears the OpenAI mark
- Droid wears Factory's real mark
- The composer's agent button wears the harness's logo
- The header runs the whole width over the session rail

### Faster

- Stop re-rendering the whole window on unrelated UI changes

### Fixed

- Keep the scroll-to-bottom button above a tall composer
- Remove the band and shadow around prompt cards
- Removed colors now take effect (Larrier preset, Reset)
- Import queued prompts that carry images
- Complete Droid session import
- One to-do card per message
- Complete OpenCode session import
- Offer sign-in when OpenCode's provider rejects the key
- Readable Hermes tool output, session usage, running state
- Complete Pi session import
- ACP tool output from terminals, tool images, refined titles
- Complete Codex rollout import
- Complete Cursor session import
- Codex stream titles, MCP images, errors and resume usage
- Cursor stream reconnects, usage, to-dos and tool shapes
- Keep list keys unique when a source repeats ids
- Codex app attachments and git directives
- Show local images in replies
- Say why a CLI failed instead of its wrapper text
- Only git directives become Codex notices
- Show pasted images that are gone by name
- A reply that is done stops looking like it is being written
- A thought with nothing in it does not open
- A session and its subagent tabs share one view
- Imported sessions update quietly, in place and in time
- Back never gets stuck on a deleted session
- The background-tasks card makes room instead of covering the chat
- Cursor follow-ups no longer fail on the model
- A call that never ran says so
- Every Cursor chat is imported once
- Claude's "This computer" sessions belong in Home
- A session running in another app is never run here at the same time
- A chat still working in Cursor shows as running
- Sessions follow the folder their agent moved to
- A project made inside a scratch folder is its own project

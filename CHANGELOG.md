# Changelog

Every Larrier release and what changed in it. Versions follow
[Semantic Versioning](https://semver.org): the major number for breaking
changes, the minor for new features, the patch for fixes. Previews carry a
suffix such as `-beta.2`.

## 0.1.39 — 2026-11-03

### Fixed

- Show a question the agent is waiting on once
- Ask questions, approvals and plans above the input only
## 0.1.38 — 2026-11-02

### New

- One text size for all of Larrier's text

### Fixed

- Open menus, dialogs and tooltips without resizing them
- Leave the side panel as it is when opening a new chat
- Open an agent's browser as a regular tab
## 0.1.37 — 2026-11-01

### Fixed

- Check and download updates without a GitHub token
## 0.1.36 — 2026-11-01

### Fixed

- Show Larrier's icon in Linux notifications
## 0.1.35 — 2026-11-01

### New

- Run on macOS and Linux
- Install and update on macOS and Linux

### Fixed

- Keep API keys on Linux after logging out
## 0.1.34 — 2026-11-01

### Fixed

- Give narration back to chats Larrier ran before 0.1.33
## 0.1.33 — 2026-11-01

### Fixed

- Show narration as reply text instead of thinking
## 0.1.32 — 2026-11-01

### New

- Look up each agent's latest version and update command
- Install, update and flag outdated agents in Agents

## 0.1.31 — 2026-11-01

### New

- Restore chats, panels and terminals after a restart
- Persist page searches, filters and unfinished forms

### Fixed

- Allow sending a queued follow-up while agent is stopped

## 0.1.30 — 2026-11-01

### New

- Choose a model's context size in its Edit menu

### Look and feel

- Mark the active agent in the model picker with a check

## 0.1.29 — 2026-11-01

### Fixed

- Clear the New Agent draft when sending
- Make Back return to the view Settings was opened from
- Open website in the browser and rename Usage section

## 0.1.28 — 2026-11-01

### New

- Choose the account a chat runs as in the model picker

### Fixed

- Share one account list between Accounts and Agents

## 0.1.27 — 2026-11-01

### Fixed

- Show sign-in state and email for each account
- Run /login and /logout in a terminal
- Skip Claude Code local commands when importing sessions
- Fill the avatar with the profile photo everywhere

## 0.1.26 — 2026-11-01

### Fixed

- Keep the current login as default when adding another

## 0.1.25 — 2026-11-01

### New

- Add a Profile settings tab with usage statistics

## 0.1.24 — 2026-11-01

### New

- Add an existing agent folder as an account
- Show models first and add Edit and Reset to the picker
- Always show the agent mode beside the environment
- Animate search expanding from its icon

### Fixed

- Avoid transient menu scrollbars and clarify environment menu
- Mark the prompt being read in the prompt timeline

## 0.1.23 — 2026-11-01

### Fixed

- Refresh plan limits periodically

## 0.1.22 — 2026-10-31

### New

- Keep drafts, side panel and browser state per chat
- List unsent New Agent messages as drafts
- Insert selected agent text as quote objects
- Add Ctrl+Shift+V to paste long text as plain text
- Show added and removed line counts on tool calls

### Fixed

- Check for new releases every ten minutes

## 0.1.21 — 2026-10-31

### Fixed

- Don't treat background tasks as an active turn

## 0.1.20 — 2026-10-31

### Fixed

- Check for updates periodically and install on exit
- Render the context bar as one connected line

## 0.1.19 — 2026-10-31

### Fixed

- Refresh tab titles for tabs opened from a link

## 0.1.18 — 2026-10-31

### New

- Run the Browser tab on Google Chrome when installed
- Open new tabs on the New Tab page
- Drag a browser page into a chat as context
- Read Claude Code's context usage breakdown
- Show plan usage and context as two rings under the box

## 0.1.17 — 2026-10-31

### Fixed

- Restore the composer height and style its text copy at once
- Center the drawn cursor in one-line fields
- Stop the prompt card sliding when editing it
- Place the page on its panel at any zoom level

## 0.1.16 — 2026-10-31

### New

- Host real WebView2 pages and add the agent browser tools
- Add annotate mode to point at page elements for the agent

## 0.1.15 — 2026-10-31

### New

- Allow taking back a message sent into a running turn

### Fixed

- Use only Larrier's own loaders and drop the dots loader

## 0.1.14 — 2026-10-31

### New

- Unify loaders into one library by purpose
- Show mentions and commands as objects inside the text
- Offer spelling suggestions in the context menu
- Hide chats of a harness that is turned off

### Fixed

- Draw a thin text cursor placed correctly in RTL and mixed text
- Keep a sent message in place when editing it
- Wrap long file names and scroll wide diffs in the Changes tab
- Keep the side panel's + menu on screen

## 0.1.13 — 2026-10-30

### New

- Copy, save and open images from the context menu

### Fixed

- Stop dropping a message sent after a background command
- Show the chat's agent in the session rail, not the composer
- Hide the drawn text cursor when a dialog or menu covers it
- Stack right-click menus above dialogs and the image viewer

## 0.1.12 — 2026-10-30

### New

- Add a browser tool set for agents over MCP
- Stream the agents' browser live in the Browser tab

## 0.1.11 — 2026-10-30

### New

- Connect OpenCode and Hermes over ACP by default

## 0.1.10 — 2026-10-30

### New

- Run Codex through its app server with live approvals

### Fixed

- Vary the request card header by the kind of request
- Use the chosen permissions for the sandbox in plan mode
- Lay out the mode and permission chips in one row

## 0.1.9 — 2026-10-30

### New

- Show MCP servers a Claude agent defines for itself
- Run a chat as one of the user's Claude Code agents
- Choose the agent a new chat runs as from @

### Look and feel

- Align reply rows with the prompt's text

### Fixed

- Keep the @ menu in view and open it where it fits
- Glide the typing cursor only along a single line
- Persist opened projects across launches
- Wait for Claude Code's MCP servers before listing them

## 0.1.8 — 2026-10-30

### New

- Read Claude Code's commands, subagents and MCP servers
- Show MCP servers that agents report themselves
- List agent commands and subagents, add /mcp and /usage

## 0.1.7 — 2026-10-30

### New

- Draw an animated typing cursor in text fields

## 0.1.6 — 2026-10-30

### New

- Show the update button only when an update exists

## 0.1.5 — 2026-10-30

### Fixed

- Restore prompt attachments when a chat re-syncs
- Align steps with the reply's left edge
- Make the code block Copy button copy

## 0.1.4 — 2026-10-30

### New

- Show a steered message immediately as pending

### Fixed

- Keep menus inside the window when their contents grow
- Let text fields handle Ctrl+Backspace

## 0.1.3 — 2026-10-30

### New

- Add per-agent on/off switches and a clearer agent card

## 0.1.2 — 2026-10-30

### New

- Answer a running agent live, steer it and queue messages
- Report what an agent refused because it could not ask
- Keep pane toggles fixed and animate pane transitions
- Replace system tooltips with themed in-app tooltips
- Order chats by recent activity
- Redraw the mark's stem as a dry brush stroke

### Look and feel

- Drop the focus ring and glow

### Fixed

- Push with the GitHub CLI's login in release.ps1
- Resume imported chats from their own workspace folder
- Read Claude Code's model menu from Claude Code itself
- Edit a sent prompt in place, keeping its attachments
- Align turn rows and count only background terminals

## 0.1.1 — 2026-10-30

### New

- Add semantic versions, update channels and release notes

## 0.1.0 — 2026-10-30

The first release.

### New

- Initial Larrier workspace
- Track subagents and background tasks end to end
- Add appearance, behavior and sidebar options
- Add tabs, split view, drag and drop and usage settings
- Add model registry with context windows and prices
- Update sessions live from other tools' session stores
- Keep context menus inside the window
- Show model names, context and price in the picker
- Read turn state, queued prompts and images
- Treat sessions running in other tools as active
- Render every tool call, including MCP calls
- Turn @-mentions and /-skills into chips
- Animate streaming steps and scrolling
- Open the prompt timeline on hover
- Use each agent's own modes and permission settings
- Handle every Claude Code record type
- Handle every stream-json event from Claude Code
- Label task, cron and team tools
- Replace the loader glyph with a smooth spinner
- Use a fixed sort order and mark unread chats
- Reword tool steps and reasoning labels
- Import Claude Code mode, pull requests and relocation
- Show the pull request link in the chat header
- Show images returned by shell commands
- Import images from Gemini CLI chats
- Show one session per window with subagents as tabs
- Add keep/preview side panel tabs and background tasks
- Add collapsible session rail with a summary
- Update plan limits live from Claude Code events
- Re-import sessions when the importer version changes
- Drop a chat by pointer position instead of an options panel
- Record how full the model's context is
- Add a session rail with context, to-dos and changes
- Mark tool calls that never ran
- Replace the logo with a stencil L mark in two tiles
- Show prompt timeline cards on hover
- Add per-kind text sizes with stepper controls
- Show prompt image thumbnails above text, cap height
- Fit panel widths to the window on resizer double-click
- Magnify timeline dashes under the pointer
- Answer agent questions in the chat for every agent
- Stream tool calls live while the model writes them
- Forward subagent text, record its model and context
- Add background work chips, cards and panel
- Show agent terminals read-only in the Terminal tab
- Edit an earlier prompt in place
- Add Mark all as read
- Add a Button Corners appearance option
- Group chats by Project and Home, add Mixed grouping
- Highlight edit card previews by file language
- Install, update and uninstall as a regular Windows app
- Read each agent's own models, defaults and providers
- Pass reasoning effort and custom endpoints at launch
- Manage models per agent, synced with each agent

### Look and feel

- Order the model registry and keep names readable
- Pin edit card, soften shadows, fade above composer
- Remove the block caret after streaming text
- Make the background tasks card smaller and lighter
- Use the OpenAI mark for Codex
- Use Factory's own mark for Droid
- Span the chat header across the window over the rail

### Faster

- Avoid re-rendering the whole window on unrelated UI changes

### Fixed

- Keep scroll-to-bottom button above tall composer
- Remove band and shadow around prompt cards
- Apply removed colors when switching appearance presets
- Import queued Claude Code prompts that carry images
- Complete Droid session import
- Show one to-do card per message
- Complete OpenCode session import
- Offer sign-in when the provider rejects the key
- Show readable tool output, usage and running state
- Complete Pi session import
- Handle ACP terminal output, tool images and titles
- Complete Codex rollout import
- Complete Cursor session import
- Clean stream titles, MCP images, errors and resume usage
- Dedupe reconnect output and handle more stream shapes
- Keep list keys unique when a source repeats ids
- Handle Codex app attachments and git directives
- Inline local images in replies
- Report the real reason when a CLI fails
- Turn only git directives into notices
- Show pasted images whose file is gone by name
- Stop showing a finished reply as still being written
- Render empty reasoning blocks as a plain row
- Keep one view for a session and its subagent tabs
- Update imported sessions in place without false activity
- Prevent Back from getting stuck on a deleted session
- Lay out the background tasks card above the chips
- Stop failing follow-ups on the reported model name
- Label calls that were blocked, rejected or stopped
- Import each Cursor chat once
- File Claude "This computer" sessions under Home
- Do not run a session that is active in another app
- Detect a chat still running in Cursor
- Follow the folder an agent moved a session to
- Treat projects created inside scratch folders as projects
- Show the running agent's logo on the agent button

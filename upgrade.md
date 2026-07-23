**Project Overview: Bread Screenshot System**

### Goal
Add a maintainable, automated system to generate high-quality screenshots/renders of **all major UI views** across the Bread ecosystem. This will dramatically speed up UI development, visual regression testing, documentation, and marketing.

### Scope

**In Scope:**
- Automated screenshot generation for all major GUI components
- Support for different themes (pywal accents, light/dark if added later)
- Consistent naming and output structure
- Easy-to-run command (`bread capture --all` or similar)
- Integration with development workflow and CI (optional)

**Out of Scope (Phase 1):**
- Video/GIF capture
- Full automated visual diffing (can be Phase 2)

### Target Apps / Views

1. **breadbar**
   - Main bar (all placements)
   - Control panel (full + sections)
   - WiFi popover, media popover, etc.
   - Notifications

2. **breadman**
   - All sidebar views (All, Upcoming, Todo, Reminder, etc.)
   - Note cards in different states
   - Editor / create flow

3. **breadbox**
   - Main launcher view
   - Different contexts

4. **bos-settings**
   - All major panels

5. **breadpad** (capture popup)

6. **breadlock** (lock screen states)

7. **Widgets** (test module that renders many widget examples)

### Technical Approach (Most Idiomatic)

**Core Components:**

1. **Shared Library** (`bread-screenshots` crate in bread-ecosystem)
   - Common screenshot utilities
   - Window finding / targeting logic (using `gtk` or `grim`)
   - Theme forcing

2. **Per-App Screenshot Mode**
   - Add `--screenshot <view>` flag to each GTK app
   - Special runtime mode that opens the desired view and calls capture after render

3. **Orchestrator**
   - A small Rust binary (`bread-capture`) or bash + Rust hybrid
   - Launches each app with proper flags, waits, captures, saves

4. **Output Structure**
   ```
   screenshots/
   ├── v0.8.0/
   │   ├── breadbar-main.png
   │   ├── breadbar-control.png
   │   ├── breadman-all.png
   │   ├── breadman-todo.png
   │   └── ...
   └── latest/ (symlinks)
   ```

### Recommended Implementation Steps

1. Create `bread-screenshots` crate in bread-ecosystem
2. Add screenshot support to the most important apps first (breadman + breadbar)
3. Build the orchestrator tool
4. Add `bread capture` subcommand to the CLI
5. Document usage + add to CONTRIBUTING.md

### Benefits

- Much faster UI iteration
- Visual regression testing
- Always up-to-date marketing/docs screenshots
- Easier contributor onboarding for UI work
- Professional polish for the project

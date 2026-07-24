with open('crates/terminal_view/src/terminal_view.rs', 'r') as f:
    content = f.read()

# Add to serialization
old_serialize = """        let workspace_id = self.workspace_id?;
        let cwd = terminal.working_directory();
        let custom_title = self.custom_title.clone();
        self.needs_serialize = false;

        let db = TerminalDb::global(cx);
        Some(cx.background_spawn(async move {
            if let Some(cwd) = cwd {
                db.save_working_directory(item_id, workspace_id, cwd)
                    .await?;
            }
            db.save_custom_title(item_id, workspace_id, custom_title)
                .await?;
            Ok(())
        }))"""
new_serialize = """        let workspace_id = self.workspace_id?;
        let cwd = terminal.working_directory();
        let custom_title = self.custom_title.clone();
        
        let shell = terminal.shell();
        let (shell_program, shell_args) = match shell {
            task::Shell::System => (None, None),
            task::Shell::Program(p) => (Some(p.clone()), None),
            task::Shell::WithArguments { program, args, .. } => (Some(program.clone()), Some(args.join("\0"))),
        };
        
        self.needs_serialize = false;

        let db = TerminalDb::global(cx);
        Some(cx.background_spawn(async move {
            if let Some(cwd) = cwd {
                db.save_working_directory(item_id, workspace_id, cwd)
                    .await?;
            }
            db.save_shell(item_id, workspace_id, shell_program, shell_args)
                .await?;
            db.save_custom_title(item_id, workspace_id, custom_title)
                .await?;
            Ok(())
        }))"""
content = content.replace(old_serialize, new_serialize)

with open('crates/terminal_view/src/terminal_view.rs', 'w') as f:
    f.write(content)

with open('crates/terminal_view/src/terminal_view.rs', 'r') as f:
    content = f.read()

# 1. Update serialization
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
        
        // Extract shell
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

# 2. Update deserialization
old_deserialize = """                    let custom_title = db
                        .get_custom_title(item_id, workspace_id)
                        .log_err()
                        .flatten()
                        .filter(|title| !title.trim().is_empty());
                    (cwd, custom_title)"""

new_deserialize = """                    let custom_title = db
                        .get_custom_title(item_id, workspace_id)
                        .log_err()
                        .flatten()
                        .filter(|title| !title.trim().is_empty());
                        
                    let shell_data = db.get_shell(item_id, workspace_id).log_err();
                    let shell = match shell_data {
                        Some((Some(program), None)) => Some(task::Shell::Program(program)),
                        Some((Some(program), Some(args_str))) => {
                            let args: Vec<String> = args_str.split('\0').map(|s| s.to_string()).collect();
                            Some(task::Shell::WithArguments { program, args, title_override: None })
                        },
                        _ => None,
                    };
                        
                    (cwd, custom_title, shell)"""

content = content.replace(old_deserialize, new_deserialize)
content = content.replace('let (cwd, custom_title) = cx', 'let (cwd, custom_title, shell) = cx')
content = content.replace('.unwrap_or((None, None));', '.unwrap_or((None, None, None));')
content = content.replace('let terminal = project\n                .update(cx, |project, cx| project.create_terminal_shell(cwd, cx))',
                          'let terminal = project\n                .update(cx, |project, cx| {\n                    if let Some(shell) = shell {\n                        // Need to somehow pass shell\n                    }\n                    project.create_terminal_shell(cwd, cx)\n                })')

with open('crates/terminal_view/src/terminal_view.rs', 'w') as f:
    f.write(content)

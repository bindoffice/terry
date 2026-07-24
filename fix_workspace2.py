with open('crates/terminal_view/src/terminal_view.rs', 'r') as f:
    content = f.read()

# Only change the shell of the terminal AFTER creating it
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
content = content.replace('let mut view = TerminalView::new(', 'if let Some(shell) = shell {\n                        // HACK: Recreate task if shell was customized.\n                        // Ideally we pass shell config to `create_terminal_shell` but that requires refactoring `project::terminals::create_terminal_shell`\n                    }\n                    let mut view = TerminalView::new(')

with open('crates/terminal_view/src/terminal_view.rs', 'w') as f:
    f.write(content)

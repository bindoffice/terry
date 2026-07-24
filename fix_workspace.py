import re
import os

with open('crates/workspace/src/workspace.rs', 'r') as f:
    content = f.read()

content = content.replace('project.create_terminal_shell(working_directory, cx)', 'project.create_terminal_shell(working_directory, None, cx)')
with open('crates/workspace/src/workspace.rs', 'w') as f:
    f.write(content)

with open('crates/terminal_view/src/terminal_panel.rs', 'r') as f:
    content = f.read()
content = content.replace('project.create_terminal_shell(working_directory, cx)', 'project.create_terminal_shell(working_directory, None, cx)')
with open('crates/terminal_view/src/terminal_panel.rs', 'w') as f:
    f.write(content)

with open('crates/terminal_view/src/terminal_view.rs', 'r') as f:
    content = f.read()
content = content.replace('.update(cx, |project, cx| project.create_terminal_shell(cwd, cx))', '.update(cx, |project, cx| project.create_terminal_shell(cwd, shell, cx))')
content = content.replace('if let Some(shell) = shell {\n                        // Need to somehow pass shell\n                    }\n                    project.create_terminal_shell(cwd, cx)\n                })',
                          'project.create_terminal_shell(cwd, shell, cx)\n                })')
with open('crates/terminal_view/src/terminal_view.rs', 'w') as f:
    f.write(content)

import re

with open('crates/terminal_view/src/persistence.rs', 'r') as f:
    content = f.read()

# Add a database migration for shell and shell_args
migration = """
        sql! (
            ALTER TABLE terminals ADD COLUMN shell TEXT;
            ALTER TABLE terminals ADD COLUMN shell_args TEXT;
        ),
"""
content = content.replace('ALTER TABLE terminals ADD COLUMN custom_title TEXT;\n        ),', 'ALTER TABLE terminals ADD COLUMN custom_title TEXT;\n        ),' + migration)

with open('crates/terminal_view/src/persistence.rs', 'w') as f:
    f.write(content)

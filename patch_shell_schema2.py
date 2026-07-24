with open('crates/terminal_view/src/persistence.rs', 'r') as f:
    content = f.read()

shell_methods = """
    pub async fn save_shell(
        &self,
        item_id: ItemId,
        workspace_id: WorkspaceId,
        shell: Option<String>,
        shell_args: Option<String>,
    ) -> Result<()> {
        self.write(move |conn| {
            let query = "INSERT INTO terminals (item_id, workspace_id, shell, shell_args)
                VALUES (?1, ?2, ?3, ?4)
                ON CONFLICT (workspace_id, item_id) DO UPDATE SET
                    shell = excluded.shell,
                    shell_args = excluded.shell_args";
            let mut statement = Statement::prepare(conn, query)?;
            let mut next_index = statement.bind(&item_id, 1)?;
            next_index = statement.bind(&workspace_id, next_index)?;
            next_index = statement.bind(&shell, next_index)?;
            statement.bind(&shell_args, next_index)?;
            statement.exec()
        })
        .await
    }

    query! {
        pub fn get_shell(item_id: ItemId, workspace_id: WorkspaceId) -> Result<(Option<String>, Option<String>)> {
            SELECT shell, shell_args
            FROM terminals
            WHERE item_id = ? AND workspace_id = ?
        }
    }
"""

content = content.replace('pub fn get_custom_title', shell_methods + '\n    pub fn get_custom_title')

with open('crates/terminal_view/src/persistence.rs', 'w') as f:
    f.write(content)

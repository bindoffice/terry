import sqlite3

conn = sqlite3.connect(':memory:')
conn.execute("CREATE TABLE terminals (workspace_id INTEGER, item_id INTEGER, working_directory BLOB, working_directory_path TEXT, PRIMARY KEY(workspace_id, item_id))")

try:
    conn.execute("INSERT INTO terminals(item_id, workspace_id, working_directory, working_directory_path) VALUES (1, 1, 1, 1) ON CONFLICT DO UPDATE SET working_directory = 1")
    print("SUCCESS")
except Exception as e:
    print(f"ERROR: {e}")

import re

content = open("src/terminal_list_panel.rs").read()

bad_timer = """        cx.spawn(|this, mut cx| async move {
            let mut interval = std::time::Duration::from_secs(5);
            loop {
                cx.background_executor().timer(interval).await;
                if let Some(this) = this.upgrade() {
                    this.update(&mut cx, |this, cx| {
                        this.save_session(cx);
                    });
                } else {
                    break;
                }
            }
        }).detach();"""

content = content.replace(bad_timer, "")
open("src/terminal_list_panel.rs", "w").write(content)

import re

with open('crates/terminal/src/alacritty.rs', 'r') as f:
    content = f.read()
    
content = content.replace('UnboundedSender', 'async_channel::Sender')

with open('crates/terminal/src/alacritty.rs', 'w') as f:
    f.write(content)

with open('crates/terminal/src/terminal.rs', 'r') as f:
    content = f.read()

content = content.replace('self.events_rx.next() =>', 'self.events_rx.recv() =>')
content = content.replace('while let Some(event) = self.events_rx.next().await {', 'while let Ok(event) = self.events_rx.recv().await {')
content = content.replace('if let Some(event) = event {', 'if let Ok(event) = event {')

with open('crates/terminal/src/terminal.rs', 'w') as f:
    f.write(content)

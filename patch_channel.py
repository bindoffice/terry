import re

def process_terminal():
    with open('crates/terminal/src/terminal.rs', 'r') as f:
        content = f.read()

    # Imports
    content = content.replace('channel::mpsc::{UnboundedReceiver, unbounded}', 'channel::mpsc::{UnboundedReceiver, unbounded}')
    content = content.replace('events_rx: UnboundedReceiver<PtyEvent>', 'events_rx: async_channel::Receiver<PtyEvent>')
    content = content.replace('events_tx: futures::channel::mpsc::UnboundedSender<PtyEvent>', 'events_tx: async_channel::Sender<PtyEvent>')
    content = content.replace('let (events_tx, events_rx) = unbounded();', 'let (events_tx, events_rx) = async_channel::bounded(1024);')
    
    # Consumption
    content = content.replace('self.events_rx.next().await', 'self.events_rx.recv().await.ok()')
    
    # Send
    content = content.replace('.unbounded_send(', '.try_send(')
    
    with open('crates/terminal/src/terminal.rs', 'w') as f:
        f.write(content)

def process_alacritty():
    with open('crates/terminal/src/alacritty.rs', 'r') as f:
        content = f.read()
        
    content = content.replace('futures::channel::mpsc::UnboundedSender', 'async_channel::Sender')
    content = content.replace('.unbounded_send(', '.try_send(')
    
    with open('crates/terminal/src/alacritty.rs', 'w') as f:
        f.write(content)

process_terminal()
process_alacritty()

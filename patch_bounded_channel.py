import re

def process_file(filepath):
    with open(filepath, 'r') as f:
        content = f.read()

    # Replace UnboundedSender/Receiver with Sender/Receiver
    content = content.replace('UnboundedSender', 'Sender')
    content = content.replace('UnboundedReceiver', 'Receiver')
    
    # In terminal.rs, replace the specific unbounded() call
    if 'terminal.rs' in filepath:
        content = content.replace('let (events_tx, events_rx) = unbounded();', 'let (events_tx, events_rx) = channel(1024);')
        # Also need to add channel to use futures::channel::mpsc::{...};
        content = content.replace('channel::mpsc::{UnboundedReceiver, unbounded}', 'channel::mpsc::{Receiver, channel, Sender}')

    content = content.replace('.unbounded_send', '.try_send')

    if 'alacritty.rs' in filepath:
        content = content.replace('futures::channel::mpsc::UnboundedSender', 'futures::channel::mpsc::Sender')

    with open(filepath, 'w') as f:
        f.write(content)

process_file('crates/terminal/src/terminal.rs')
process_file('crates/terminal/src/alacritty.rs')

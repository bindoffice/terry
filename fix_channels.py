import re

with open('crates/terminal/src/terminal.rs', 'r') as f:
    content = f.read()

content = content.replace('let (events_tx, events_rx) = unbounded();', 'let (events_tx, events_rx) = async_channel::bounded(1024);')
content = content.replace('events_rx: UnboundedReceiver<PtyEvent>', 'events_rx: async_channel::Receiver<PtyEvent>')
content = content.replace('events_tx: futures::channel::mpsc::UnboundedSender<PtyEvent>', 'events_tx: async_channel::Sender<PtyEvent>')
content = content.replace('self.events_rx.next() =>', 'self.events_rx.recv().fuse() =>')
content = content.replace('while let Some(event) = self.events_rx.next().await {', 'while let Ok(event) = self.events_rx.recv().await {')
content = content.replace('if let Some(event) = event {', 'if let Ok(event) = event {')
content = content.replace('.unbounded_send(', '.try_send(')

# Add dirty field
content = content.replace('pub last_content: Content,\n    pub selection_head: Option<Point>,', 'pub last_content: Content,\n    pub content_dirty: bool,\n    pub selection_head: Option<Point>,')
content = content.replace('last_content: Content::default(),\n            selection_head: None,', 'last_content: Content::default(),\n            content_dirty: true,\n            selection_head: None,')
content = content.replace('last_content,\n            breadcrumb_text:', 'last_content,\n            content_dirty: true,\n            breadcrumb_text:')

wakeup_block = """            TerminalBackendEvent::Wakeup => {
                self.detect_init_command_startup_marker();"""
wakeup_block_new = """            TerminalBackendEvent::Wakeup => {
                self.content_dirty = true;
                self.detect_init_command_startup_marker();"""
content = content.replace(wakeup_block, wakeup_block_new)

sync_old = """    pub fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let term = self.term.clone();
        let mut terminal = term.lock_unfair();
        //Note that the ordering of events matters for event processing
        while let Some(e) = self.events.pop_front() {
            self.process_terminal_event(&e, &mut terminal, window, cx)
        }

        self.last_content = make_content(&terminal, &self.last_content);
    }"""
    
sync_new = """    pub fn sync(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if !self.content_dirty && self.events.is_empty() {
             return;
        }

        let term = self.term.clone();
        let mut terminal = term.lock_unfair();
        //Note that the ordering of events matters for event processing
        while let Some(e) = self.events.pop_front() {
            self.process_terminal_event(&e, &mut terminal, window, cx)
        }

        self.last_content = make_content(&terminal, &self.last_content);
        self.content_dirty = false;
    }"""
content = content.replace(sync_old, sync_new)

with open('crates/terminal/src/terminal.rs', 'w') as f:
    f.write(content)

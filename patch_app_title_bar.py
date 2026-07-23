with open("src/app_title_bar.rs", "r") as f:
    text = f.read()

text = text.replace("use ui::{Color, IconButton, IconName, IconSize, Label, LabelSize, Tooltip, prelude::*};", "use ui::{IconButton, IconName, IconSize, Tooltip, prelude::*};")

with open("src/app_title_bar.rs", "w") as f:
    f.write(text)

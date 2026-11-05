---
name: Output that dtk did not summarize
about: Data that dtk left alone, or read wrongly. You send the layout, not the data.
labels: shape
---

**What the output is.** For example: the command that printed it, the tool and its version.

**What you expected.** For example: a table of rows, a log digest, an outline.

**The layout.** Run this on the output (a file, or `-` for standard input) and paste the result.
Every digit becomes `9` and every letter `a` or `A`, so no value leaves your machine. Read it
before you paste it.

```
dtk skeleton FILE --command "the command that printed it"
```

Use `--keep-header` if the column names are not private; they help a lot.

**Did it change anything?** `dtk explain FILE --command "..."` says what dtk made of the output.
You can paste that too.

**How big is it?** About how many lines, and how often you see output like this.

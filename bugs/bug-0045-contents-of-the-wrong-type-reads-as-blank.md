# Bug: A Page Whose `/Contents` Is the Wrong Type Reads as Blank, So `--require-text` Passes It

**Severity:** contract (fails open, on malformed input)

## What happens

`helpers::page_content_ids` returns `None` when a page’s `/Contents` is a direct object that is neither a reference nor an array (an integer, a name, a direct dictionary), and it drops, without a word, any element of a `/Contents` array that is not a reference.  `read_content_streams` then has nothing to read, so `paint::page_paint` sees a page that draws nothing: `lacks_text()` is false, the page is not listed in `pages_without_text`, and `--require-text` passes it as blank.

The same wrong type behind a reference is read as unreadable (`unread`) and fails, so the two spellings of one fault give opposite answers.

## What should happen

`/Contents` may only be a stream (always indirect) or an array of them (PDF 32000-1 §7.7.3.3, Table 31).  A value of any other type, or an array element that is not a reference, says the page’s content cannot be read, not that it is empty: the page should count as `unread`, which `--text` lists and `--require-text` fails (an unreadable thing is never an empty one).  A null `/Contents`, or a reference to nothing, is no entry (§7.3.10), so a page with it is blank as now.

## Reproduction

A synthetic lopdf page with `/Contents 5` (a direct integer) beside a page with text: `pdf-dump f.pdf --text --require-text` exits 0 and lists nothing.  With `/Contents 9 0 R`, where object 9 is the integer 5, it exits 1 naming the page as unread.

## Notes

Found during the review of plan-0015; it predates that change (plan-0012, v0.37.0).  `page_content_ids` is shared by the extractors, so the fix should report the fault to its callers rather than change what they read: `read_content_streams` already carries an `unread` flag.

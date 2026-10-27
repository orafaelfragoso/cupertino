# cupertino-typeahead

Type-to-select for menus and pop-up lists, with no dependencies. `TypeAhead` keeps the text typed so far and finds the
next item whose title starts with it, ignoring case.

- A pause of one second starts a new search.
- One typed character looks after the highlighted item, so repeating it steps through the items that share it.
- Two or more characters search from the highlighted item, wrapping.
- Items that cannot be chosen (separators, disabled rows) are passed as `None` and never match.
- A space is part of the text while a search is under way (`typing`); the caller treats it as a choice otherwise.

Used by `cupertino-settings` (choice menus) and `cupertino-files` (context menus).

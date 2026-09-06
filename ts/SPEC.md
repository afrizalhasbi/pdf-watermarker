This is an application to add watermarks on pdfs. It uses tauri webviews.

Here are the list of features:
- load a pdf. if all page not same size: error popup.
- preview the first page on the screen on some area on the screen
- input a text
- drag and drop a text to designate the desired location wrt to a page
- confirm, then put the text on the same location at every page except for the first and last

Layout
--------------------------------------------
|            ___________        |          | <- side bar
|            |         |        |Text      |
|            |         |        |[input]   |
|            | [page ] |        |          |
|            | preview]|        |          |
|            |         |        |          |
|            |_________|        | [Finish] |
|                               |          |
--------------------------------------------

the page preview area is a "load pdf" file picker button when a pdf is no yet loaded

"Text" is a caption. the [input] text is cached in localstorage so every boot it retrieves from there

Style
Minimalist, not neon crap, just light mode with grays and whites

Questions to keep in mind:
- transplanting the logical watermark text position from the preview to the real location?
- library or crate to do this? we're using rust after all
- How hard to roll an RGBA color picker for the text?

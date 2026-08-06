# Attachments

Parent: [Interactive TUI](/interactive-tui).

Press `ctrl+v` to paste a clipboard image as an attachment when a supported host helper is available (`wl-paste`/`xclip` on Linux, `pngpaste` on macOS, or PowerShell on Windows/WSL). Hosts such as [Herdr](/integrations/herdr) may paste clipboard content as a single filesystem path. Rho loads PNG, JPEG, GIF, and WebP paths as image attachments. It also extracts text from UTF-8 text and source files, PDFs, DOCX documents, and XLSX, XLS, or ODS spreadsheets and queues the result as a document attachment. An absolute document path is handled before slash-command parsing, so paths beginning with `/` do not become unknown commands. Press backspace in an empty message box to remove the last queued file.

Document extraction is bounded by input and extracted-character limits. PDFs need a text layer because scanned-image OCR is not included. PDF headings, lists, tables, links, and reading order are preserved as structured Markdown. The model receives extracted text with the filename, MIME type, truncation state, and warnings. Session model history stores that bounded text and metadata, not raw PDF or Office bytes. Images continue to use the provider's multimodal image path.

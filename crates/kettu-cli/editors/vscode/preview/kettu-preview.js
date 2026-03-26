// Syntax highlighting for kettu code blocks in the markdown preview
(function () {
    const KEYWORDS = /\b(?:package|use|as|version|feature|interface|record|variant|enum|flags|type|resource|func|static|constructor|world|import|export|include|let|return|if|else|match|while|for|in|to|downto|step|break|continue|async|await|map|filter|reduce|assert|true|false|none|some|ok|err)\b/g;
    const LINE_COMMENT = /\/\/.*$/gm;
    const BLOCK_COMMENT = /\/\*[\s\S]*?\*\//g;
    const STRING = /"[^"]*"/g;
    const NUMBER = /\b[0-9]+\b/g;
    const DECORATOR = /@(?:test|since|unstable|deprecated)\b/g;

    function escapeHtml(text) {
        return text.replace(/&/g, '&amp;').replace(/</g, '&lt;').replace(/>/g, '&gt;');
    }

    function highlight(code) {
        // Tokenize by splitting into segments that are either special or plain text
        const tokens = [];
        let remaining = code;
        let pos = 0;

        // Collect all matches with their positions and types
        const matches = [];
        const patterns = [
            { regex: BLOCK_COMMENT, cls: 'kettu-comment' },
            { regex: LINE_COMMENT, cls: 'kettu-comment' },
            { regex: STRING, cls: 'kettu-string' },
            { regex: DECORATOR, cls: 'kettu-decorator' },
            { regex: KEYWORDS, cls: 'kettu-keyword' },
            { regex: NUMBER, cls: 'kettu-number' },
        ];

        for (const { regex, cls } of patterns) {
            regex.lastIndex = 0;
            let m;
            while ((m = regex.exec(code)) !== null) {
                matches.push({ start: m.index, end: m.index + m[0].length, text: m[0], cls });
            }
        }

        // Sort by position, longer matches first for ties
        matches.sort((a, b) => a.start - b.start || b.end - a.end);

        // Build highlighted HTML, skipping overlapping matches
        let result = '';
        let cursor = 0;
        for (const m of matches) {
            if (m.start < cursor) continue; // skip overlapping
            if (m.start > cursor) {
                result += escapeHtml(code.slice(cursor, m.start));
            }
            result += `<span class="${m.cls}">${escapeHtml(m.text)}</span>`;
            cursor = m.end;
        }
        if (cursor < code.length) {
            result += escapeHtml(code.slice(cursor));
        }
        return result;
    }

    function highlightBlocks() {
        document.querySelectorAll('code.language-kettu, code.language-wit').forEach(block => {
            if (block.dataset.kettuHighlighted) return;
            block.innerHTML = highlight(block.textContent);
            block.dataset.kettuHighlighted = 'true';
        });
    }

    // Run on load and watch for DOM updates (preview re-renders)
    if (document.readyState === 'loading') {
        document.addEventListener('DOMContentLoaded', highlightBlocks);
    } else {
        highlightBlocks();
    }

    const observer = new MutationObserver(highlightBlocks);
    observer.observe(document.body, { childList: true, subtree: true });
})();

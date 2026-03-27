const path = require('path');

function normalizePath(filePath) {
    return path.resolve(filePath);
}

function hasBreakpointInRange(breakpointsMap, filePath, startLine, endLine) {
    return getBreakpointLinesInRange(breakpointsMap, filePath, startLine, endLine).length > 0;
}

function getBreakpointLinesInRange(breakpointsMap, filePath, startLine, endLine) {
    const normalized = normalizePath(filePath);
    const lines = breakpointsMap.get(normalized);
    if (!lines || lines.size === 0) {
        return [];
    }

    const start = Number.isInteger(startLine) ? startLine : 0;
    const end = Number.isInteger(endLine) ? endLine : start;
    const min = Math.min(start, end);
    const max = Math.max(start, end);

    const hits = [];
    for (const line of lines) {
        if (line >= min && line <= max) {
            hits.push(line);
        }
    }

    hits.sort((a, b) => a - b);
    return hits;
}

module.exports = {
    normalizePath,
    hasBreakpointInRange,
    getBreakpointLinesInRange,
};

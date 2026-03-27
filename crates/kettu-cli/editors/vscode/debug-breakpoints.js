const path = require('path');

function normalizePath(filePath) {
    return path.resolve(filePath);
}

function hasBreakpointInRange(breakpointsMap, filePath, startLine, endLine) {
    const normalized = normalizePath(filePath);
    const lines = breakpointsMap.get(normalized);
    if (!lines || lines.size === 0) {
        return false;
    }

    const start = Number.isInteger(startLine) ? startLine : 0;
    const end = Number.isInteger(endLine) ? endLine : start;
    const min = Math.min(start, end);
    const max = Math.max(start, end);

    for (const line of lines) {
        if (line >= min && line <= max) {
            return true;
        }
    }

    return false;
}

module.exports = {
    normalizePath,
    hasBreakpointInRange,
};

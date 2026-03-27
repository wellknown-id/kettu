function stripLineComment(line) {
    const index = line.indexOf('//');
    if (index >= 0) {
        return line.slice(0, index);
    }
    return line;
}

function trimOuterParens(expr) {
    let text = expr.trim();
    while (text.startsWith('(') && text.endsWith(')')) {
        text = text.slice(1, -1).trim();
    }
    return text;
}

function splitByOperator(expr, operator) {
    const token = ` ${operator} `;
    const index = expr.indexOf(token);
    if (index < 0) {
        return null;
    }
    return [expr.slice(0, index), expr.slice(index + token.length)];
}

function evalExpr(rawExpr, env) {
    const expr = trimOuterParens(rawExpr);

    if (/^-?\d+$/.test(expr)) {
        return Number(expr);
    }

    if (expr === 'true') return true;
    if (expr === 'false') return false;

    if (/^".*"$/.test(expr)) {
        return expr.slice(1, -1);
    }

    if (/^[A-Za-z_][A-Za-z0-9_-]*$/.test(expr)) {
        return Object.prototype.hasOwnProperty.call(env, expr) ? env[expr] : undefined;
    }

    const operators = ['||', '&&', '==', '!=', '>=', '<=', '>', '<', '+', '-', '*', '/'];
    for (const operator of operators) {
        const parts = splitByOperator(expr, operator);
        if (!parts) continue;

        const left = evalExpr(parts[0], env);
        const right = evalExpr(parts[1], env);
        if (typeof left === 'undefined' || typeof right === 'undefined') {
            return undefined;
        }

        switch (operator) {
            case '+': return left + right;
            case '-': return left - right;
            case '*': return left * right;
            case '/': return right === 0 ? undefined : left / right;
            case '==': return left == right; // eslint-disable-line eqeqeq
            case '!=': return left != right; // eslint-disable-line eqeqeq
            case '>': return left > right;
            case '<': return left < right;
            case '>=': return left >= right;
            case '<=': return left <= right;
            case '&&': return Boolean(left && right);
            case '||': return Boolean(left || right);
            default: return undefined;
        }
    }

    return undefined;
}

function collectVisibleLocals(sourceText, startLine, stopLine) {
    const lines = sourceText.split(/\r?\n/);
    const env = {};

    const first = Math.max(1, startLine);
    const last = Math.min(lines.length, stopLine);

    for (let lineNo = first; lineNo <= last; lineNo += 1) {
        const line = stripLineComment(lines[lineNo - 1]).trim();
        if (!line) continue;

        const letMatch = line.match(/^let\s+([A-Za-z_][A-Za-z0-9_-]*)\s*=\s*(.+);$/);
        if (letMatch) {
            const name = letMatch[1];
            const value = evalExpr(letMatch[2], env);
            if (typeof value !== 'undefined') {
                env[name] = value;
            }
            continue;
        }

        const assignMatch = line.match(/^([A-Za-z_][A-Za-z0-9_-]*)\s*=\s*(.+);$/);
        if (assignMatch) {
            const name = assignMatch[1];
            const value = evalExpr(assignMatch[2], env);
            if (typeof value !== 'undefined') {
                env[name] = value;
            }
        }
    }

    return env;
}

module.exports = {
    collectVisibleLocals,
};

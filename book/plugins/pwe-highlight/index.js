const hljs = require("highlight.js");

const KEYWORDS = [
    // sections
    "world", "funcs", "systems",
    // world items
    "gravity", "title", "params", "chan", "value", "entity", "field", "pool",
    "soft", "struct", "width", "height", "depth", "dx", "nx", "ny", "nz",
    "spacing", "origin", "shape", "part",
    // entity fields
    "position", "velocity", "state", "vec", "mass", "dynamic", "nbody",
    "parent", "restitution", "friction", "box", "sphere", "hull", "rotation",
    "camera", "color", "size", "opacity", "glow", "label", "orient", "vector",
    // shape parts
    "point", "capsule", "svg", "at", "scale", "faces",
    // control
    "return", "let", "repeat", "until", "while", "for", "in", "break",
    "continue", "if",
    // logic
    "and", "or", "not", "true", "false",
    // modules
    "import", "from", "as",
].join(" ");

const BUILTINS = [
    "sin", "cos", "exp", "ln", "sqrt", "abs", "floor", "ceil", "round", "sign",
    "log10", "log2", "sinh", "cosh", "tanh", "asin", "acos", "atan", "pow",
    "atan2", "hypot", "min", "max", "if", "random", "noise", "print", "emit",
    "last_event", "at", "periodic", "schedule", "active", "inte", "vlen",
    "vdot", "vdist", "neighbor_count", "nearest_dist", "neighbor_mean",
    "nearest_dx", "nearest_dy", "nearest_dz", "fget", "fset", "flap",
].join(" ");

if (!hljs.getLanguage("pwe")) {
    hljs.registerLanguage("pwe", (h) => ({
        name: "PWE",
        keywords: { keyword: KEYWORDS, built_in: BUILTINS },
        contains: [
            h.COMMENT("#", "$"),
            h.COMMENT("//", "$"),
            h.NUMBER_MODE,
            h.QUOTE_STRING_MODE,
            // unit annotations: [m/s^2]
            { className: "meta", begin: /\[[A-Za-z0-9*/^]+\]/ },
            // constants read as atoms, not keywords
            {
                className: "literal",
                begin: /(^|[^A-Za-z0-9_])(pi|e)(?![A-Za-z0-9_])/,
            },
        ],
    }));
}

module.exports = {};

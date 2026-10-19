// SPDX-License-Identifier: AGPL-3.0-only
//! Coding practice: write a JavaScript function, which is then run on test
//! cases in the app's sandbox (QuickJS: no files, no network, a time limit)
//! and compared with a reference solution's answers. The cases are made
//! fresh from a seed each time, with the edge cases that catch most bugs
//! (empty input, repeats, negatives, ties).

use std::time::Duration;

use serde_json::Value;

use super::tasks::{Check, Rng, Skill, Task};

pub struct Problem {
    pub level: u32,
    pub name: &'static str,
    pub params: &'static str,
    /// What it does, after "that".
    pub ask: &'static str,
    /// The example's arguments (a JS array).
    pub example: &'static str,
    /// A JS expression: the test cases, as an array of argument lists.
    pub cases: &'static str,
    pub reference: &'static str,
}

/// Helpers the test cases are made with (only in the reference run).
const HELPERS: &str = r#"
var __s = __SEED__ >>> 0;
function rnd() { __s = (__s + 0x6D2B79F5) >>> 0; var t = __s; t = Math.imul(t ^ (t >>> 15), t | 1); t ^= t + Math.imul(t ^ (t >>> 7), t | 61); return ((t ^ (t >>> 14)) >>> 0) / 4294967296; }
function int(a, b) { return a + Math.floor(rnd() * (b - a + 1)); }
function ints(n, a, b) { var r = []; for (var i = 0; i < n; i++) r.push(int(a, b)); return r; }
function times(n, f) { var r = []; for (var i = 0; i < n; i++) r.push(f(i)); return r; }
function pick(xs) { return xs[int(0, xs.length - 1)]; }
var WORDS = ['the', 'cat', 'sat', 'on', 'a', 'mat', 'big', 'red', 'dog', 'ran', 'to', 'park', 'and', 'blue', 'sky', 'sun', 'moon', 'tree', 'house', 'river', 'light', 'stone', 'apple', 'green', 'fast', 'slow', 'old', 'new'];
function phrase(n, caps) { return times(n, function () { var w = pick(WORDS); return caps && rnd() < 0.3 ? w[0].toUpperCase() + w.slice(1) : w; }).join(' '); }
function letters(n, from) { from = from || 'abcdefghijklmnopqrstuvwxyz'; var s = ''; for (var i = 0; i < n; i++) s += from[int(0, from.length - 1)]; return s; }
function shuffle(s) { var a = s.split(''); for (var i = a.length - 1; i > 0; i--) { var j = int(0, i); var t = a[i]; a[i] = a[j]; a[j] = t; } return a.join(''); }
function roman(n) { var v = [1000, 900, 500, 400, 100, 90, 50, 40, 10, 9, 5, 4, 1], s = ['M', 'CM', 'D', 'CD', 'C', 'XC', 'L', 'XL', 'X', 'IX', 'V', 'IV', 'I'], r = ''; for (var i = 0; i < v.length; i++) while (n >= v[i]) { r += s[i]; n -= v[i]; } return r; }
function nest(d) { return times(int(0, 4), function () { return d > 0 && rnd() < 0.35 ? nest(d - 1) : int(0, 9); }); }
function brackets(n) { var open = '([{', close = ')]}', st = [], s = ''; for (var i = 0; i < n; i++) { if (st.length && rnd() < 0.45) s += close[st.pop()]; else { var k = int(0, 2); st.push(k); s += open[k]; } if (rnd() < 0.2) s += letters(1); } while (st.length) s += close[st.pop()]; return s; }
function runs() { return times(int(1, 6), function () { var c = letters(1, 'abc'); return c.repeat(int(1, 12)); }).join(''); }
function increasing() { var x = int(0, 5), r = []; for (var i = int(1, 12); i > 0; i--) { r.push(x); x += rnd() < 0.6 ? 1 : int(2, 4); } return r; }
function camel() { var w = ['user', 'name', 'get', 'set', 'parse', 'value', 'item', 'count', 'load', 'file'], a = ['HTTP', 'URL', 'ID', 'JSON', 'XML', 'API']; var s = pick(w); if (rnd() < 0.3) s = s[0].toUpperCase() + s.slice(1); times(int(1, 3), function () { if (rnd() < 0.35) s += pick(a); else { var x = pick(w); s += x[0].toUpperCase() + x.slice(1); } }); if (rnd() < 0.2) s += int(1, 9); return s; }
function mutate(w) { var a = w.split(''); times(int(0, 3), function () { var k = int(0, 2), i = int(0, a.length); if (k === 0) a.splice(i, 0, letters(1)); else if (a.length && k === 1) a.splice(Math.min(i, a.length - 1), 1); else if (a.length) a[Math.min(i, a.length - 1)] = letters(1); }); return a.join(''); }
function expr(d) { if (d <= 0 || rnd() < 0.3) return String(int(0, 20)); var r = rnd(); if (r < 0.25) return '(' + expr(d - 1) + ')'; if (r < 0.45) return expr(d - 1) + ' / ' + int(1, 9); return expr(d - 1) + ' ' + pick(['+', '-', '*']) + ' ' + expr(d - 1); }
function encoded(d) { return times(int(1, 3), function () { return d > 0 && rnd() < 0.5 ? int(1, 4) + '[' + encoded(d - 1) + ']' : letters(int(1, 3), 'abcxyz'); }).join(''); }
function distinct(n, a, b) { var r = []; while (r.length < n) { var x = int(a, b); if (r.indexOf(x) < 0) r.push(x); } return r; }
function distinctStrs(n, a, b, from) { var r = [], tries = 0; while (r.length < n && tries++ < 200) { var w = letters(int(a, b), from); if (r.indexOf(w) < 0) r.push(w); } return r; }
function regexCase() { var toks = times(int(1, 5), function () { return pick(['a', 'b', '.']) + (rnd() < 0.4 ? '*' : ''); }); var s = toks.map(function (t) { var one = function () { return t[0] === '.' ? letters(1, 'ab') : t[0]; }; if (t.length > 1) { var r = ''; for (var k = int(0, 3); k > 0; k--) r += one(); return r; } return one(); }).join(''); if (rnd() < 0.4) s = mutate(s).replace(/[^ab]/g, 'a'); return [s, toks.join('')]; }
function wildCase() { var s = letters(int(1, 8), 'abc'), p = s.split('').map(function (c) { var r = rnd(); return r < 0.2 ? '?' : r < 0.35 ? '*' : c; }).join(''); if (rnd() < 0.3) p = mutate(p).replace(/[^abc?*]/g, 'b'); return [s, p]; }
"#;

/// Puts objects' keys in order and rounds fractions, so answers compare
/// the same however they were built.
const CANON: &str = "function (v) { var c = function (v) { if (Array.isArray(v)) return v.map(c); if (v && typeof v === 'object') { var o = {}; Object.keys(v).sort().forEach(function (k) { o[k] = c(v[k]); }); return o; } if (typeof v === 'number' && !Number.isInteger(v)) return Number(v.toPrecision(10)); return v; }; return c(v); }";

pub const PROBLEMS: &[Problem] = &[
    // ----- level 1 -----
    Problem {
        level: 1,
        name: "sumEvens",
        params: "nums",
        ask: "returns the sum of the even numbers in the array (0 if there are none)",
        example: "[[1, 2, 3, 4, 10]]",
        cases: "[[[]], [[1, 3, 5]], [[-4, -3, 2]]].concat(times(6, function () { return [ints(int(1, 12), -30, 30)]; }))",
        reference: "function sumEvens(nums) { return nums.filter(function (n) { return n % 2 === 0; }).reduce(function (a, b) { return a + b; }, 0); }",
    },
    Problem {
        level: 1,
        name: "countVowels",
        params: "text",
        ask: "returns how many vowels (a, e, i, o, u, in upper or lower case) the text has",
        example: "[\"Hello World\"]",
        cases: "[[''], ['rhythm']].concat(times(6, function () { return [phrase(int(2, 6), true)]; }))",
        reference: "function countVowels(text) { return (text.match(/[aeiou]/gi) || []).length; }",
    },
    Problem {
        level: 1,
        name: "reverseWords",
        params: "sentence",
        ask: "returns the sentence with its words in reverse order, separated by single spaces (the input has single spaces between words)",
        example: "[\"the quick brown fox\"]",
        cases: "[[''], ['one']].concat(times(6, function () { return [phrase(int(2, 7))]; }))",
        reference: "function reverseWords(sentence) { return sentence.split(' ').reverse().join(' '); }",
    },
    Problem {
        level: 1,
        name: "isPalindrome",
        params: "text",
        ask: "returns true if the text reads the same backwards, ignoring case, spaces and punctuation",
        example: "[\"A man, a plan, a canal: Panama\"]",
        cases: "[[''], ['Was it a car or a cat I saw?'], ['No lemon, no melon'], ['hello'], ['ab']].concat(times(4, function () { var w = letters(int(2, 6)); return [rnd() < 0.5 ? w + w.split('').reverse().join('') : w + letters(2)]; }))",
        reference: "function isPalindrome(text) { var t = text.toLowerCase().replace(/[^a-z0-9]/g, ''); return t === t.split('').reverse().join(''); }",
    },
    Problem {
        level: 1,
        name: "fizzBuzz",
        params: "n",
        ask: "returns an array of strings for the numbers 1 to n: \"Fizz\" for multiples of 3, \"Buzz\" for multiples of 5, \"FizzBuzz\" for multiples of both, and otherwise the number itself as a string",
        example: "[5]",
        cases: "[[1], [15]].concat(times(4, function () { return [int(2, 40)]; }))",
        reference: "function fizzBuzz(n) { var r = []; for (var i = 1; i <= n; i++) r.push(i % 15 === 0 ? 'FizzBuzz' : i % 3 === 0 ? 'Fizz' : i % 5 === 0 ? 'Buzz' : String(i)); return r; }",
    },
    Problem {
        level: 1,
        name: "capitalizeWords",
        params: "text",
        ask: "returns the text with the first letter of every word made a capital and the rest of each word left as it is (words are separated by single spaces)",
        example: "[\"hello big world\"]",
        cases: "[[''], ['a']].concat(times(6, function () { return [phrase(int(1, 6), true)]; }))",
        reference: "function capitalizeWords(text) { return text.split(' ').map(function (w) { return w ? w[0].toUpperCase() + w.slice(1) : w; }).join(' '); }",
    },
    Problem {
        level: 1,
        name: "countOccurrences",
        params: "items, value",
        ask: "returns how many times value appears in the array (compared with ===)",
        example: "[[1, 2, 1, 3, 1], 1]",
        cases: "[[[], 5]].concat(times(6, function () { return [ints(int(1, 12), 0, 5), int(0, 6)]; }))",
        reference: "function countOccurrences(items, value) { return items.filter(function (x) { return x === value; }).length; }",
    },
    Problem {
        level: 1,
        name: "maxDifference",
        params: "nums",
        ask: "returns the largest number in the array minus the smallest, or 0 for an empty array",
        example: "[[4, 9, 1, 7]]",
        cases: "[[[]], [[5]]].concat(times(6, function () { return [ints(int(2, 10), -50, 50)]; }))",
        reference: "function maxDifference(nums) { return nums.length ? Math.max.apply(null, nums) - Math.min.apply(null, nums) : 0; }",
    },
    // ----- level 2 -----
    Problem {
        level: 2,
        name: "uniqueSorted",
        params: "nums",
        ask: "returns the distinct numbers of the array, sorted from smallest to largest",
        example: "[[10, 2, 33, 2, 5]]",
        cases: "[[[]], [[100, 20, 3]]].concat(times(6, function () { return [ints(int(3, 12), -20, 200)]; }))",
        reference: "function uniqueSorted(nums) { return Array.from(new Set(nums)).sort(function (a, b) { return a - b; }); }",
    },
    Problem {
        level: 2,
        name: "chunk",
        params: "items, size",
        ask: "splits the array into groups of size items, in order; the last group may be shorter, and an empty array gives []",
        example: "[[1, 2, 3, 4, 5], 2]",
        cases: "[[[], 3], [[1, 2, 3], 5]].concat(times(6, function () { return [ints(int(1, 11), 0, 9), int(1, 4)]; }))",
        reference: "function chunk(items, size) { var r = []; for (var i = 0; i < items.length; i += size) r.push(items.slice(i, i + size)); return r; }",
    },
    Problem {
        level: 2,
        name: "runLength",
        params: "text",
        ask: "compresses each run of the same character into the character and the run's length, so \"aaabcc\" becomes \"a3b1c2\" (an empty string stays empty)",
        example: "[\"aaabcc\"]",
        cases: "[[''], ['a']].concat(times(6, function () { return [runs()]; }))",
        reference: "function runLength(text) { var out = '', i = 0; while (i < text.length) { var j = i; while (j < text.length && text[j] === text[i]) j++; out += text[i] + (j - i); i = j; } return out; }",
    },
    Problem {
        level: 2,
        name: "secondLargest",
        params: "nums",
        ask: "returns the second largest distinct value in the array, or null if there isn't one",
        example: "[[5, 1, 5, 3]]",
        cases: "[[[]], [[7]], [[4, 4, 4]]].concat(times(5, function () { return [ints(int(2, 9), -10, 20)]; }))",
        reference: "function secondLargest(nums) { var u = Array.from(new Set(nums)).sort(function (a, b) { return b - a; }); return u.length < 2 ? null : u[1]; }",
    },
    Problem {
        level: 2,
        name: "rotateRight",
        params: "items, k",
        ask: "returns a new array with the items moved k places to the right, wrapping around (k may be larger than the length; an empty array gives [])",
        example: "[[1, 2, 3, 4, 5], 2]",
        cases: "[[[], 3], [[1, 2, 3], 0]].concat(times(6, function () { return [ints(int(1, 8), 0, 9), int(0, 20)]; }))",
        reference: "function rotateRight(items, k) { var n = items.length; if (!n) return []; k %= n; return items.slice(n - k).concat(items.slice(0, n - k)); }",
    },
    Problem {
        level: 2,
        name: "isAnagram",
        params: "a, b",
        ask: "returns true if the two words use exactly the same letters the same number of times, ignoring case",
        example: "[\"Listen\", \"Silent\"]",
        cases: "[['', '']].concat(times(7, function () { var w = letters(int(3, 8)), r = rnd(); if (r < 0.4) return [w, shuffle(w)]; if (r < 0.7) return [w, shuffle(w).slice(1) + 'z']; return [w.toUpperCase(), shuffle(w)]; }))",
        reference: "function isAnagram(a, b) { var f = function (s) { return s.toLowerCase().split('').sort().join(''); }; return f(a) === f(b); }",
    },
    Problem {
        level: 2,
        name: "wordCounts",
        params: "text",
        ask: "returns an object mapping each word to how many times it appears; words are runs of the letters a to z after making the text lowercase, and everything else separates words",
        example: "[\"The cat and the hat.\"]",
        cases: "[['']].concat(times(6, function () { return [times(int(3, 10), function () { var w = pick(WORDS.slice(0, 8)); return (rnd() < 0.3 ? w.toUpperCase() : w) + pick([' ', ' ', ', ', '. ', '! ']); }).join('')]; }))",
        reference: "function wordCounts(text) { var o = {}; (text.toLowerCase().match(/[a-z]+/g) || []).forEach(function (w) { o[w] = (o[w] || 0) + 1; }); return o; }",
    },
    Problem {
        level: 2,
        name: "flatten",
        params: "nested",
        ask: "returns a flat array of all the numbers in a nested array, in order, however deep the nesting goes",
        example: "[[1, [2, [3, [4]], 5]]]",
        cases: "[[[]], [[[[]]]]].concat(times(6, function () { return [nest(3)]; }))",
        reference: "function flatten(nested) { var r = []; (function go(a) { a.forEach(function (x) { if (Array.isArray(x)) go(x); else r.push(x); }); })(nested); return r; }",
    },
    Problem {
        level: 2,
        name: "transpose",
        params: "matrix",
        ask: "returns the matrix (an array of rows, all the same length) with its rows and columns swapped; an empty matrix gives []",
        example: "[[[1, 2, 3], [4, 5, 6]]]",
        cases: "[[[]], [[[7]]]].concat(times(6, function () { var r = int(1, 4), c = int(1, 4); return [times(r, function () { return ints(c, 0, 9); })]; }))",
        reference: "function transpose(matrix) { if (!matrix.length) return []; return matrix[0].map(function (_, j) { return matrix.map(function (row) { return row[j]; }); }); }",
    },
    // ----- level 3 -----
    Problem {
        level: 3,
        name: "balanced",
        params: "text",
        ask: "returns true if every bracket (), [] and {} in the text is closed in the right order; other characters are ignored",
        example: "[\"(a[b]{c})\"]",
        cases: "[[''], ['('], [')('], ['([)]'], ['a(b)c]']].concat(times(5, function () { var s = brackets(int(2, 8)); return [rnd() < 0.4 ? shuffle(s) : s]; }))",
        reference: "function balanced(text) { var st = [], pairs = { ')': '(', ']': '[', '}': '{' }; for (var i = 0; i < text.length; i++) { var c = text[i]; if ('([{'.indexOf(c) >= 0) st.push(c); else if (pairs[c]) { if (st.pop() !== pairs[c]) return false; } } return st.length === 0; }",
    },
    Problem {
        level: 3,
        name: "romanToInt",
        params: "roman",
        ask: "converts a Roman numeral (I, V, X, L, C, D, M, including subtractive forms like IV, XC and CM) to a number",
        example: "[\"MCMXCIV\"]",
        cases: "[['IV'], ['MMMCMXCIX']].concat(times(6, function () { return [roman(int(1, 3999))]; }))",
        reference: "function romanToInt(roman) { var v = { I: 1, V: 5, X: 10, L: 50, C: 100, D: 500, M: 1000 }, t = 0; for (var i = 0; i < roman.length; i++) { var a = v[roman[i]], b = v[roman[i + 1]] || 0; t += a < b ? -a : a; } return t; }",
    },
    Problem {
        level: 3,
        name: "intToRoman",
        params: "num",
        ask: "converts a number from 1 to 3999 to a Roman numeral, using subtractive forms like IV, XC and CM",
        example: "[1994]",
        cases: "[[4], [9], [3999]].concat(times(5, function () { return [int(1, 3999)]; }))",
        reference: "function intToRoman(num) { var v = [1000, 900, 500, 400, 100, 90, 50, 40, 10, 9, 5, 4, 1], s = ['M', 'CM', 'D', 'CD', 'C', 'XC', 'L', 'XL', 'X', 'IX', 'V', 'IV', 'I'], r = ''; for (var i = 0; i < v.length; i++) while (num >= v[i]) { r += s[i]; num -= v[i]; } return r; }",
    },
    Problem {
        level: 3,
        name: "mergeRanges",
        params: "ranges",
        ask: "takes an array of [start, end] pairs in any order and returns them sorted by start, with ranges that overlap or touch (one starts where another ends) merged into one",
        example: "[[[8, 10], [1, 3], [2, 6], [10, 12]]]",
        cases: "[[[]]].concat(times(7, function () { return [times(int(1, 6), function () { var a = int(0, 30); return [a, a + int(0, 8)]; })]; }))",
        reference: "function mergeRanges(ranges) { var r = ranges.map(function (x) { return x.slice(); }).sort(function (a, b) { return a[0] - b[0]; }), out = []; r.forEach(function (x) { var last = out[out.length - 1]; if (last && x[0] <= last[1]) last[1] = Math.max(last[1], x[1]); else out.push(x); }); return out; }",
    },
    Problem {
        level: 3,
        name: "commonPrefix",
        params: "words",
        ask: "returns the longest start that all the words share, or an empty string if there is none (or the array is empty)",
        example: "[[\"flower\", \"flow\", \"flight\"]]",
        cases: "[[[]], [['alone']], [['dog', 'racecar']]].concat(times(5, function () { var p = letters(int(0, 4)); return [times(int(2, 4), function () { return p + letters(int(0, 4)); })]; }))",
        reference: "function commonPrefix(words) { if (!words.length) return ''; var p = words[0]; words.forEach(function (w) { while (w.indexOf(p) !== 0) p = p.slice(0, -1); }); return p; }",
    },
    Problem {
        level: 3,
        name: "compressRanges",
        params: "nums",
        ask: "takes a sorted array of distinct non-negative integers and returns them as a comma-separated string, writing each run of three or more consecutive numbers as start-end; runs of two are written as two numbers. So [1, 2, 3, 5, 7, 8] becomes \"1-3,5,7,8\"",
        example: "[[0, 1, 2, 4, 6, 7, 8, 9, 11]]",
        cases: "[[[]], [[4]], [[4, 5]], [[1, 2, 3]]].concat(times(4, function () { return [increasing()]; }))",
        reference: "function compressRanges(nums) { var out = [], i = 0; while (i < nums.length) { var j = i; while (j + 1 < nums.length && nums[j + 1] === nums[j] + 1) j++; if (j - i >= 2) out.push(nums[i] + '-' + nums[j]); else for (var k = i; k <= j; k++) out.push(String(nums[k])); i = j + 1; } return out.join(','); }",
    },
    Problem {
        level: 3,
        name: "toSnakeCase",
        params: "name",
        ask: "converts a camelCase or PascalCase name to snake_case. A run of capitals is one word (like HTTP), except that its last capital starts a new word when a lowercase letter follows it, and digits stay with what comes before them. So \"parseHTTPResponse\" becomes \"parse_http_response\" and \"userID2\" becomes \"user_id2\"",
        example: "[\"loadJSONFile\"]",
        cases: "[['x'], ['getX']].concat(times(6, function () { return [camel()]; }))",
        reference: "function toSnakeCase(name) { return name.replace(/([a-z0-9])([A-Z])/g, '$1_$2').replace(/([A-Z]+)([A-Z][a-z])/g, '$1_$2').toLowerCase(); }",
    },
    Problem {
        level: 3,
        name: "spiral",
        params: "matrix",
        ask: "returns the values of a matrix (an array of rows, all the same length) in clockwise spiral order, starting at the top left; an empty matrix gives []",
        example: "[[[1, 2, 3], [4, 5, 6], [7, 8, 9]]]",
        cases: "[[[]], [[[1]]], [[[1, 2, 3]]], [[[1], [2], [3]]]].concat(times(4, function () { var r = int(2, 5), c = int(2, 5); return [times(r, function () { return ints(c, 0, 99); })]; }))",
        reference: "function spiral(matrix) { var out = []; if (!matrix.length) return out; var top = 0, bottom = matrix.length - 1, left = 0, right = matrix[0].length - 1; while (top <= bottom && left <= right) { for (var j = left; j <= right; j++) out.push(matrix[top][j]); top++; for (var i = top; i <= bottom; i++) out.push(matrix[i][right]); right--; if (top <= bottom) { for (j = right; j >= left; j--) out.push(matrix[bottom][j]); bottom--; } if (left <= right) { for (i = bottom; i >= top; i--) out.push(matrix[i][left]); left++; } } return out; }",
    },
    Problem {
        level: 3,
        name: "nextGreater",
        params: "nums",
        ask: "returns an array where each number is replaced by the first number to its right that is larger than it, or -1 if there is none",
        example: "[[2, 7, 3, 5, 4, 6, 8]]",
        cases: "[[[]]].concat(times(7, function () { return [ints(int(1, 12), 0, 20)]; }))",
        reference: "function nextGreater(nums) { return nums.map(function (x, i) { for (var j = i + 1; j < nums.length; j++) if (nums[j] > x) return nums[j]; return -1; }); }",
    },
    // ----- level 4 -----
    Problem {
        level: 4,
        name: "editDistance",
        params: "a, b",
        ask: "returns the smallest number of single-letter insertions, deletions or replacements that turn string a into string b",
        example: "[\"kitten\", \"sitting\"]",
        cases: "[['', ''], ['abc', ''], ['', 'xy']].concat(times(5, function () { var w = letters(int(1, 9), 'abcde'); return [w, mutate(w)]; }))",
        reference: "function editDistance(a, b) { var d = []; for (var i = 0; i <= a.length; i++) { d[i] = [i]; for (var j = 1; j <= b.length; j++) d[i][j] = i === 0 ? j : Math.min(d[i - 1][j] + 1, d[i][j - 1] + 1, d[i - 1][j - 1] + (a[i - 1] === b[j - 1] ? 0 : 1)); } return d[a.length][b.length]; }",
    },
    Problem {
        level: 4,
        name: "coinChange",
        params: "coins, amount",
        ask: "returns the fewest coins that add up to exactly amount, where each coin value in the array can be used any number of times; 0 for an amount of 0, and -1 if it can't be done",
        example: "[[1, 2, 5], 11]",
        cases: "[[[2], 3], [[1], 0], [[1, 3, 4], 6]].concat(times(5, function () { return [distinct(int(1, 4), 2, 25), int(1, 99)]; }))",
        reference: "function coinChange(coins, amount) { var best = [0]; for (var a = 1; a <= amount; a++) { best[a] = Infinity; coins.forEach(function (c) { if (c <= a && best[a - c] + 1 < best[a]) best[a] = best[a - c] + 1; }); } return best[amount] === Infinity ? -1 : best[amount]; }",
    },
    Problem {
        level: 4,
        name: "longestIncreasing",
        params: "nums",
        ask: "returns the length of the longest strictly increasing subsequence of the array (its numbers don't have to be next to each other)",
        example: "[[10, 9, 2, 5, 3, 7, 101, 18]]",
        cases: "[[[]], [[5, 5, 5]]].concat(times(6, function () { return [ints(int(3, 14), 0, 30)]; }))",
        reference: "function longestIncreasing(nums) { var l = nums.map(function () { return 1; }), best = 0; for (var i = 0; i < nums.length; i++) { for (var j = 0; j < i; j++) if (nums[j] < nums[i]) l[i] = Math.max(l[i], l[j] + 1); best = Math.max(best, l[i]); } return best; }",
    },
    Problem {
        level: 4,
        name: "evaluate",
        params: "expr",
        ask: "evaluates an arithmetic expression made of non-negative whole numbers, + - * /, parentheses and spaces, with the usual order of operations. Every division truncates toward zero straight away (7 / 2 is 3, and -7 / 2 is -3). Don't use eval or Function",
        example: "[\"2 + 3 * (7 - 1) / 4\"]",
        cases: "[['7 / 2 * 2'], ['(3 - 10) / 2'], ['1 + 2 * 3']].concat(times(5, function () { return [expr(3)]; }))",
        reference: "function evaluate(expr) { var t = expr.match(/\\d+|[-+*/()]/g) || [], i = 0; function atom() { var x = t[i++]; if (x === '(') { var v = sum(); i++; return v; } if (x === '-') return -atom(); return Number(x); } function product() { var v = atom(); while (t[i] === '*' || t[i] === '/') { var op = t[i++], w = atom(); v = op === '*' ? v * w : Math.trunc(v / w); } return v; } function sum() { var v = product(); while (t[i] === '+' || t[i] === '-') { var op = t[i++], w = product(); v = op === '+' ? v + w : v - w; } return v; } return sum(); }",
    },
    Problem {
        level: 4,
        name: "decodeString",
        params: "s",
        ask: "expands every k[text] in the string into text repeated k times, where the patterns may be nested: \"3[a2[c]]\" becomes \"accaccacc\" and \"2[ab]x\" becomes \"ababx\"",
        example: "[\"3[a2[c]]\"]",
        cases: "[[''], ['abc'], ['2[ab]3[c]']].concat(times(5, function () { return [encoded(2)]; }))",
        reference: "function decodeString(s) { var st = [['', 1]], num = 0; for (var i = 0; i < s.length; i++) { var c = s[i]; if (c >= '0' && c <= '9') num = num * 10 + Number(c); else if (c === '[') { st.push(['', num]); num = 0; } else if (c === ']') { var top = st.pop(); st[st.length - 1][0] += top[0].repeat(top[1]); } else st[st.length - 1][0] += c; } return st[0][0]; }",
    },
    Problem {
        level: 4,
        name: "topKFrequent",
        params: "nums, k",
        ask: "returns the k numbers that appear most often in the array, most frequent first; when two appear equally often, the smaller number comes first",
        example: "[[1, 1, 1, 2, 2, 3], 2]",
        cases: "times(8, function () { var a = ints(int(4, 15), 0, 6); return [a, int(1, new Set(a).size)]; })",
        reference: "function topKFrequent(nums, k) { var c = new Map(); nums.forEach(function (x) { c.set(x, (c.get(x) || 0) + 1); }); return Array.from(c.keys()).sort(function (a, b) { return c.get(b) - c.get(a) || a - b; }).slice(0, k); }",
    },
    Problem {
        level: 4,
        name: "trapWater",
        params: "heights",
        ask: "takes the heights of a row of bars, each one unit wide, and returns how many units of rain water are trapped between them",
        example: "[[0, 1, 0, 2, 1, 0, 1, 3, 2, 1, 2, 1]]",
        cases: "[[[]], [[3]], [[3, 0, 3]]].concat(times(5, function () { return [ints(int(3, 14), 0, 8)]; }))",
        reference: "function trapWater(heights) { var total = 0; for (var i = 0; i < heights.length; i++) { var l = Math.max.apply(null, heights.slice(0, i + 1)), r = Math.max.apply(null, heights.slice(i)); total += Math.min(l, r) - heights[i]; } return total; }",
    },
    Problem {
        level: 4,
        name: "wrapText",
        params: "text, width",
        ask: "breaks the text (words separated by single spaces) into lines of at most width characters, breaking only between words and putting as many words as fit on each line; a word longer than width goes on a line of its own. Returns the array of lines; an empty text gives []",
        example: "[\"the quick brown fox jumps over the lazy dog\", 10]",
        cases: "[['', 5], ['tiny', 2]].concat(times(6, function () { var w = times(int(3, 12), function () { return rnd() < 0.1 ? letters(int(9, 14)) : pick(WORDS); }); return [w.join(' '), int(6, 16)]; }))",
        reference: "function wrapText(text, width) { var lines = [], cur = ''; text.split(' ').filter(Boolean).forEach(function (w) { if (!cur) cur = w; else if ((cur + ' ' + w).length <= width) cur += ' ' + w; else { lines.push(cur); cur = w; } }); if (cur) lines.push(cur); return lines; }",
    },
    // ----- level 5 -----
    Problem {
        level: 5,
        name: "numberToWords",
        params: "num",
        ask: "writes a whole number from 0 to 999,999,999 in English words, each word capitalized and separated by single spaces, with no \"and\" and no hyphens: 12345 becomes \"Twelve Thousand Three Hundred Forty Five\" and 0 becomes \"Zero\"",
        example: "[1000010]",
        cases: "[[0], [5], [13], [20], [100], [1000000]].concat(times(6, function () { return [int(0, Math.pow(10, int(1, 9)) - 1)]; }))",
        reference: "function numberToWords(num) { if (num === 0) return 'Zero'; var ones = ['', 'One', 'Two', 'Three', 'Four', 'Five', 'Six', 'Seven', 'Eight', 'Nine', 'Ten', 'Eleven', 'Twelve', 'Thirteen', 'Fourteen', 'Fifteen', 'Sixteen', 'Seventeen', 'Eighteen', 'Nineteen'], tens = ['', '', 'Twenty', 'Thirty', 'Forty', 'Fifty', 'Sixty', 'Seventy', 'Eighty', 'Ninety']; function three(n) { var w = []; if (n >= 100) { w.push(ones[Math.floor(n / 100)], 'Hundred'); n %= 100; } if (n >= 20) { w.push(tens[Math.floor(n / 10)]); n %= 10; } if (n > 0) w.push(ones[n]); return w; } var out = []; [[1e6, 'Million'], [1e3, 'Thousand']].forEach(function (u) { if (num >= u[0]) { out = out.concat(three(Math.floor(num / u[0])), [u[1]]); num %= u[0]; } }); return out.concat(three(num)).join(' '); }",
    },
    Problem {
        level: 5,
        name: "simplifyPath",
        params: "path",
        ask: "simplifies an absolute Unix-style path: \".\" means the current folder, \"..\" goes up one folder (staying at \"/\" at the top), repeated slashes count as one, and the result starts with \"/\" and has no slash at the end (unless it's just \"/\")",
        example: "[\"/a/./b/../../c/\"]",
        cases: "[['/'], ['/../'], ['//x//y/']].concat(times(5, function () { return ['/' + times(int(1, 7), function () { return pick(['a', 'b', 'docs', 'src', '..', '.', '']); }).join('/')]; }))",
        reference: "function simplifyPath(path) { var st = []; path.split('/').forEach(function (p) { if (p === '..') st.pop(); else if (p && p !== '.') st.push(p); }); return '/' + st.join('/'); }",
    },
    Problem {
        level: 5,
        name: "longestPalindrome",
        params: "s",
        ask: "returns the longest part of the string (a run of characters next to each other) that reads the same backwards; if several are equally long, the one that starts first; an empty string gives \"\"",
        example: "[\"babad\"]",
        cases: "[[''], ['a'], ['cbbd'], ['abacdfgdcaba']].concat(times(4, function () { return [letters(int(5, 14), 'abc')]; }))",
        reference: "function longestPalindrome(s) { var best = ''; for (var c = 0; c < s.length; c++) { for (var k = 0; k < 2; k++) { var l = c, r = c + k; while (l >= 0 && r < s.length && s[l] === s[r]) { l--; r++; } var p = s.slice(l + 1, r); if (p.length > best.length) best = p; } } return best; }",
    },
    Problem {
        level: 5,
        name: "groupAnagrams",
        params: "words",
        ask: "groups the words that are anagrams of each other (the same letters in any order). Each group's words are in alphabetical order (repeats kept), and the groups are ordered by their first word",
        example: "[[\"eat\", \"tea\", \"tan\", \"ate\", \"nat\", \"bat\"]]",
        cases: "[[[]]].concat(times(6, function () { var bases = times(int(2, 4), function () { return letters(int(2, 4), 'abcde'); }); return [times(int(3, 8), function () { return shuffle(pick(bases)); })]; }))",
        reference: "function groupAnagrams(words) { var m = {}; words.forEach(function (w) { var k = '#' + w.split('').sort().join(''); (m[k] = m[k] || []).push(w); }); return Object.keys(m).map(function (k) { return m[k].slice().sort(); }).sort(function (a, b) { return a[0] < b[0] ? -1 : a[0] > b[0] ? 1 : 0; }); }",
    },
    Problem {
        level: 5,
        name: "validIPv4",
        params: "s",
        ask: "returns true if the string is a valid IPv4 address: exactly four numbers from 0 to 255 separated by dots, digits only, and no leading zeros (though \"0\" itself is fine)",
        example: "[\"192.168.0.1\"]",
        cases: "[['0.0.0.0'], ['255.255.255.255'], ['256.1.1.1'], ['01.2.3.4'], ['1.2.3'], ['1.2.3.4.5'], ['1..2.3'], ['a.b.c.d'], [' 1.2.3.4'], ['1.2.3.-4']].concat(times(4, function () { return [times(4, function () { return String(int(0, 300)); }).join('.')]; }))",
        reference: "function validIPv4(s) { var p = s.split('.'); return p.length === 4 && p.every(function (x) { return /^(0|[1-9][0-9]{0,2})$/.test(x) && Number(x) <= 255; }); }",
    },
    Problem {
        level: 5,
        name: "maxMeetings",
        params: "meetings",
        ask: "takes [start, end] meetings and returns the largest number one person can attend fully, one at a time; a meeting may start at the moment another ends",
        example: "[[[1, 4], [2, 3], [3, 5], [7, 8]]]",
        cases: "[[[]], [[[1, 2], [2, 3], [3, 4]]], [[[1, 10], [2, 3], [4, 5]]]].concat(times(5, function () { return [times(int(2, 9), function () { var a = int(0, 20); return [a, a + int(1, 6)]; })]; }))",
        reference: "function maxMeetings(meetings) { var m = meetings.slice().sort(function (a, b) { return a[1] - b[1]; }), end = -Infinity, n = 0; m.forEach(function (x) { if (x[0] >= end) { n++; end = x[1]; } }); return n; }",
    },
    Problem {
        level: 5,
        name: "runningMedian",
        params: "nums",
        ask: "returns an array whose i-th value is the median of the first i + 1 numbers (for an even count, the average of the two middle numbers)",
        example: "[[5, 15, 1, 3]]",
        cases: "[[[]], [[5]]].concat(times(6, function () { return [ints(int(2, 10), -20, 40)]; }))",
        reference: "function runningMedian(nums) { var seen = []; return nums.map(function (x) { seen.push(x); var s = seen.slice().sort(function (a, b) { return a - b; }), m = s.length >> 1; return s.length % 2 ? s[m] : (s[m - 1] + s[m]) / 2; }); }",
    },
    Problem {
        level: 5,
        name: "minWindow",
        params: "s, t",
        ask: "returns the shortest run of characters in s that contains every character of t (counting repeats); if several are equally short, the one that starts first; \"\" if there is none or t is empty",
        example: "[\"ADOBECODEBANC\", \"ABC\"]",
        cases: "[['a', 'aa'], ['abc', ''], ['aa', 'aa']].concat(times(5, function () { return [letters(int(6, 16), 'abcd'), letters(int(1, 3), 'abcd')]; }))",
        reference: "function minWindow(s, t) { if (!t) return ''; var best = ''; for (var i = 0; i < s.length; i++) { var need = {}, missing = t.length; for (var k = 0; k < t.length; k++) need[t[k]] = (need[t[k]] || 0) + 1; for (var j = i; j < s.length; j++) { if (need[s[j]] > 0) missing--; if (need[s[j]] !== undefined) need[s[j]]--; if (missing === 0) { if (!best || j - i + 1 < best.length) best = s.slice(i, j + 1); break; } } } return best; }",
    },
    // ----- level 6 -----
    Problem {
        level: 6,
        name: "justify",
        params: "words, width",
        ask: "lays the words out in lines exactly width characters long. Each line takes as many words as fit with single spaces between them; then the extra spaces are spread between its words, the gaps on the left getting one more when they can't be even. The last line, and any line with one word, is left-aligned with single spaces and padded with spaces on the right. No word is longer than width; returns the array of lines",
        example: "[[\"This\", \"is\", \"an\", \"example\", \"of\", \"text\", \"justification.\"], 16]",
        cases: "[[['a'], 1], [['to', 'be', 'or', 'not', 'to', 'be'], 6]].concat(times(6, function () { return [times(int(3, 12), function () { return pick(WORDS); }), int(10, 20)]; }))",
        reference: "function justify(words, width) { var lines = [], i = 0; while (i < words.length) { var j = i, len = words[i].length; while (j + 1 < words.length && len + 1 + words[j + 1].length <= width) { j++; len += 1 + words[j].length; } var line = words.slice(i, j + 1), gaps = line.length - 1, out; if (j === words.length - 1 || gaps === 0) { out = line.join(' '); out += ' '.repeat(width - out.length); } else { var spaces = width - line.reduce(function (a, w) { return a + w.length; }, 0), base = Math.floor(spaces / gaps), extra = spaces % gaps; out = ''; line.forEach(function (w, k) { out += w; if (k < gaps) out += ' '.repeat(base + (k < extra ? 1 : 0)); }); } lines.push(out); i = j + 1; } return lines; }",
    },
    Problem {
        level: 6,
        name: "regexMatch",
        params: "s, p",
        ask: "returns true if the pattern p matches the whole string s, where \".\" matches any one character and \"*\" means zero or more of the character (or \".\") just before it. No library regular expressions",
        example: "[\"aab\", \"c*a*b\"]",
        cases: "[['aa', 'a'], ['aa', 'a*'], ['ab', '.*'], ['mississippi', 'mis*is*p*.']].concat(times(6, function () { return regexCase(); }))",
        reference: "function regexMatch(s, p) { var memo = {}; function go(i, j) { var key = i + ',' + j; if (key in memo) return memo[key]; var r; if (j === p.length) r = i === s.length; else { var first = i < s.length && (p[j] === s[i] || p[j] === '.'); if (j + 1 < p.length && p[j + 1] === '*') r = go(i, j + 2) || (first && go(i + 1, j)); else r = first && go(i + 1, j + 1); } memo[key] = r; return r; } return go(0, 0); }",
    },
    Problem {
        level: 6,
        name: "lru",
        params: "capacity, ops",
        ask: "simulates a least-recently-used cache that holds at most capacity keys. ops is a list like [[\"put\", 1, 10], [\"get\", 1]]: put stores a value (replacing any old one), get gives the value or -1 if it isn't there, and both count as using the key. When a put makes it hold more than capacity keys, the least recently used key is dropped. Returns the results of the gets, in order",
        example: "[2, [[\"put\", 1, 1], [\"put\", 2, 2], [\"get\", 1], [\"put\", 3, 3], [\"get\", 2]]]",
        cases: "[[2, [['put', 1, 1], ['put', 2, 2], ['get', 1], ['put', 3, 3], ['get', 2], ['put', 4, 4], ['get', 1], ['get', 3], ['get', 4]]]].concat(times(6, function () { return [int(1, 3), times(int(4, 12), function () { return rnd() < 0.5 ? ['put', int(1, 5), int(1, 99)] : ['get', int(1, 5)]; })]; }))",
        reference: "function lru(capacity, ops) { var m = new Map(), out = []; ops.forEach(function (op) { if (op[0] === 'get') { if (m.has(op[1])) { var v = m.get(op[1]); m.delete(op[1]); m.set(op[1], v); out.push(v); } else out.push(-1); } else { if (m.has(op[1])) m.delete(op[1]); m.set(op[1], op[2]); if (m.size > capacity) m.delete(m.keys().next().value); } }); return out; }",
    },
    Problem {
        level: 6,
        name: "wildcardMatch",
        params: "s, p",
        ask: "returns true if the pattern p matches the whole string s, where \"?\" matches any one character and \"*\" matches any run of characters, including none",
        example: "[\"adceb\", \"*a*b\"]",
        cases: "[['aa', 'a'], ['aa', '*'], ['cb', '?a'], ['acdcb', 'a*c?b']].concat(times(6, function () { return wildCase(); }))",
        reference: "function wildcardMatch(s, p) { var d = []; for (var i = 0; i <= s.length; i++) { d[i] = []; for (var j = 0; j <= p.length; j++) { if (i === 0 && j === 0) d[i][j] = true; else if (j === 0) d[i][j] = false; else if (p[j - 1] === '*') d[i][j] = d[i][j - 1] || (i > 0 && d[i - 1][j]); else d[i][j] = i > 0 && d[i - 1][j - 1] && (p[j - 1] === '?' || p[j - 1] === s[i - 1]); } } return d[s.length][p.length]; }",
    },
    Problem {
        level: 6,
        name: "shortestPaths",
        params: "n, edges, start",
        ask: "takes nodes numbered 0 to n - 1 and edges [a, b, length], roads that can be used both ways (lengths are positive), and returns the shortest distance from start to every node, with -1 for nodes that can't be reached",
        example: "[4, [[0, 1, 5], [1, 2, 3], [0, 2, 10]], 0]",
        cases: "times(8, function () { var n = int(2, 7); return [n, times(int(1, 9), function () { return [int(0, n - 1), int(0, n - 1), int(1, 20)]; }), int(0, n - 1)]; })",
        reference: "function shortestPaths(n, edges, start) { var d = [], done = []; for (var i = 0; i < n; i++) d.push(Infinity); d[start] = 0; for (var r = 0; r < n; r++) { var u = -1; for (i = 0; i < n; i++) if (!done[i] && d[i] < Infinity && (u < 0 || d[i] < d[u])) u = i; if (u < 0) break; done[u] = true; edges.forEach(function (e) { if (e[0] === u && d[u] + e[2] < d[e[1]]) d[e[1]] = d[u] + e[2]; if (e[1] === u && d[u] + e[2] < d[e[0]]) d[e[0]] = d[u] + e[2]; }); } return d.map(function (x) { return x === Infinity ? -1 : x; }); }",
    },
    Problem {
        level: 6,
        name: "wordBreakCount",
        params: "s, words",
        ask: "returns in how many different ways the string s can be split into a sequence of the given words, each usable any number of times (the list has no repeats)",
        example: "[\"catsanddog\", [\"cat\", \"cats\", \"and\", \"sand\", \"dog\"]]",
        cases: "[['aaaa', ['a', 'aa']], ['abc', ['x']]].concat(times(6, function () { var w = distinctStrs(int(2, 4), 1, 3, 'ab'); var s = times(int(2, 6), function () { return pick(w); }).join(''); return [rnd() < 0.3 ? s + 'b' : s, w]; }))",
        reference: "function wordBreakCount(s, words) { var ways = [1]; for (var i = 1; i <= s.length; i++) { ways[i] = 0; words.forEach(function (w) { if (w && i >= w.length && s.slice(i - w.length, i) === w) ways[i] += ways[i - w.length]; }); } return ways[s.length]; }",
    },
    Problem {
        level: 6,
        name: "topoOrder",
        params: "n, edges",
        ask: "takes nodes numbered 0 to n - 1 and edges [a, b] meaning a must come before b, and returns an order of all the nodes that keeps every edge, always taking the smallest-numbered node that is free to go next; [] if the edges make a cycle",
        example: "[4, [[1, 0], [2, 0], [3, 1]]]",
        cases: "[[3, [[0, 1], [1, 2], [2, 0]]]].concat(times(7, function () { var n = int(2, 7); return [n, times(int(1, 8), function () { var a = int(0, n - 1), b = int(0, n - 1); return rnd() < 0.85 && a !== b ? [Math.min(a, b), Math.max(a, b)] : [b, a]; })]; }))",
        reference: "function topoOrder(n, edges) { var indeg = [], out = [], order = []; for (var i = 0; i < n; i++) { indeg.push(0); out.push([]); } edges.forEach(function (e) { out[e[0]].push(e[1]); indeg[e[1]]++; }); while (order.length < n) { var next = -1; for (i = 0; i < n; i++) if (indeg[i] === 0 && order.indexOf(i) < 0) { next = i; break; } if (next < 0) return []; order.push(next); out[next].forEach(function (b) { indeg[b]--; }); } return order; }",
    },
    Problem {
        level: 6,
        name: "longestValidParens",
        params: "s",
        ask: "takes a string of \"(\" and \")\" and returns the length of the longest run of characters next to each other that forms correctly matched parentheses",
        example: "[\")()())\"]",
        cases: "[[''], ['(()'], ['()(())']].concat(times(5, function () { return [letters(int(4, 16), '()')]; }))",
        reference: "function longestValidParens(s) { var best = 0, st = [-1]; for (var i = 0; i < s.length; i++) { if (s[i] === '(') st.push(i); else { st.pop(); if (!st.length) st.push(i); else best = Math.max(best, i - st[st.length - 1]); } } return best; }",
    },
];

/// The test cases and the right answers, from the reference run: (cases,
/// answers, the example's answer), all as JSON text.
fn reference(p: &Problem, seed: u32) -> Option<(String, Vec<String>, String)> {
    let code = format!(
        "{helpers}\nvar __canon = {CANON};\nvar __cases = {cases};\nvar __ex = {example};\n{reference}\nJSON.stringify({{ cases: __cases, want: __cases.map(function (c) {{ return JSON.stringify(__canon({name}.apply(null, JSON.parse(JSON.stringify(c))))); }}), example: JSON.stringify(__canon({name}.apply(null, JSON.parse(JSON.stringify(__ex))))) }})",
        helpers = HELPERS.replace("__SEED__", &seed.to_string()),
        cases = p.cases,
        example = p.example,
        reference = p.reference,
        name = p.name,
    );
    let ran = crate::jsrun::run_within(&code, |_| Err(String::new()), Duration::from_secs(5));
    let v: Value = serde_json::from_str(ran.value.as_deref()?).ok()?;
    let want = v["want"].as_array()?.iter().filter_map(|w| w.as_str().map(str::to_string)).collect();
    Some((v["cases"].to_string(), want, v["example"].as_str()?.to_string()))
}

/// "chunk([1,2,3,4,5], 2)".
fn call_text(name: &str, args: &str) -> String {
    let v: Value = serde_json::from_str(args).unwrap_or(Value::Null);
    let parts: Vec<String> = v.as_array().map(|a| a.iter().map(|x| x.to_string()).collect()).unwrap_or_default();
    format!("{name}({})", parts.join(", "))
}

/// A coding task at this level.
pub fn task(level: u32, rng: &mut Rng) -> Task {
    let at_level: Vec<&Problem> = PROBLEMS.iter().filter(|p| p.level == level).collect();
    for _ in 0..4 {
        let p = *rng.pick(&at_level);
        let seed = rng.next() as u32;
        let Some((cases, want, example)) = reference(p, seed) else { continue };
        let user = format!(
            "Write a JavaScript function `{}({})` that {}.\n\nFor example, {} returns {example}.\n\nReply with only the complete function in one ```javascript code block, with no explanation. It must not print anything or read input.",
            p.name,
            p.params,
            p.ask,
            call_text(p.name, p.example),
        );
        return Task { skill: Skill::Code, level, user, tools: None, check: Check::Code { name: p.name.to_string(), cases, want } };
    }
    // The references are tested; this only happens if the sandbox can't start.
    Task { skill: Skill::Code, level, user: String::new(), tools: None, check: Check::Code { name: String::new(), cases: "[]".into(), want: Vec::new() } }
}

static FENCE: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"(?s)```[a-zA-Z]*[ \t]*\n(.*?)```").unwrap());
static EXPORT: std::sync::LazyLock<regex::Regex> = std::sync::LazyLock::new(|| regex::Regex::new(r"(?m)^\s*export\s+(default\s+)?").unwrap());

/// The code in a reply: the code block that defines the function (or the
/// last one).
pub fn code_in(text: &str, name: &str) -> Option<String> {
    let blocks: Vec<&str> = FENCE.captures_iter(text).filter_map(|c| c.get(1).map(|m| m.as_str())).collect();
    let defines = |b: &&&str| b.contains(&format!("function {name}")) || b.contains(&format!("{name} =")) || b.contains(&format!("{name}="));
    let code = blocks.iter().rev().find(defines).or(blocks.last()).copied()?;
    Some(EXPORT.replace_all(code, "").to_string())
}

/// Runs the reply's function on the cases: right only if every answer
/// matches the reference's.
pub fn passes(name: &str, cases: &str, want: &[String], reply: &str) -> bool {
    if name.is_empty() || want.is_empty() {
        return false;
    }
    let Some(code) = code_in(reply, name) else { return false };
    let program = format!(
        "var module = {{ exports: {{}} }}, exports = module.exports;\n{code}\n;(function () {{ var canon = {CANON}; var f = typeof {name} === 'function' ? {name} : module.exports && typeof module.exports.{name} === 'function' ? module.exports.{name} : typeof module.exports === 'function' ? module.exports : null; if (!f) return 'missing'; var cases = {cases}, want = {want}, bad = 0; for (var i = 0; i < cases.length; i++) {{ try {{ if (JSON.stringify(canon(f.apply(null, JSON.parse(JSON.stringify(cases[i]))))) !== want[i]) bad++; }} catch (e) {{ bad++; }} }} return bad ? 'fail ' + bad : 'pass'; }})()",
        want = serde_json::to_string(want).unwrap_or_default(),
    );
    let ran = crate::jsrun::run_within(&program, |_| Err(String::new()), Duration::from_secs(3));
    ran.error.is_none() && ran.value.as_deref() == Some("pass")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every reference runs, makes its cases, and passes its own check;
    /// a wrong answer fails it.
    #[test]
    fn every_problem_checks_itself() {
        for (i, p) in PROBLEMS.iter().enumerate() {
            for seed in [1u32, 77, 4242] {
                let (cases, want, example) = reference(p, seed).unwrap_or_else(|| panic!("{} didn't run", p.name));
                assert!(want.len() >= 5, "{}: {} cases", p.name, want.len());
                assert!(!example.is_empty());
                let right = format!("Here you go:\n```javascript\n{}\n```", p.reference);
                assert!(passes(p.name, &cases, &want, &right), "{} fails its own check", p.name);
                // Another problem's function under this name is wrong.
                let other = &PROBLEMS[(i + 1) % PROBLEMS.len()];
                let wrong = format!("```js\n{}\nvar {} = {};\n```", other.reference, p.name, other.name);
                assert!(!passes(p.name, &cases, &want, &wrong), "{} passes with {}", p.name, other.name);
            }
        }
    }

    #[test]
    fn levels_have_problems() {
        for level in 1..=super::super::tasks::LEVELS {
            assert!(PROBLEMS.iter().filter(|p| p.level == level).count() >= 6, "level {level}");
        }
    }

    #[test]
    fn reads_code_however_it_is_wrapped() {
        let r = "Sure!\n\n```js\nexport function add(a, b) { return a + b; }\n```\n\nUsage:\n```js\nconsole.log(add(1, 2));\n```";
        assert_eq!(code_in(r, "add").unwrap().trim(), "function add(a, b) { return a + b; }");
        assert!(passes("add", "[[1,2],[3,4]]", &["3".into(), "7".into()], r));
        let arrow = "```javascript\nconst add = (a, b) => a + b;\nmodule.exports = { add };\n```";
        assert!(passes("add", "[[1,2]]", &["3".into()], arrow));
        assert!(!passes("add", "[[1,2]]", &["3".into()], "function add(a, b) { return a + b; }"), "it was asked for in a code block");
        let endless = "```js\nfunction add(a, b) { while (true) {} }\n```";
        assert!(!passes("add", "[[1,2]]", &["3".into()], endless));
    }
}

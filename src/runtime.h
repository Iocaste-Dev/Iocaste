/* Iocaste runtime v0.0.5-alpha (generated programs include this text verbatim) */
#include <stdint.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <math.h>
#include <ctype.h>

typedef const char *ioc_string;
typedef struct { int64_t length; ioc_string *items; } ioc_strarr;

static void ioc_panic(const char *msg) {
    fflush(stdout);
    fprintf(stderr, "panic: %s\n", msg);
    exit(101);
}
static void *ioc_alloc(size_t n) {
    void *p = malloc(n ? n : 1);
    if (!p) ioc_panic("out of memory");
    return p;
}
static int64_t ioc_div(int64_t a, int64_t b) {
    if (b == 0) ioc_panic("division by zero");
    if (b == -1) return (int64_t)(0 - (uint64_t)a);
    return a / b;
}
static int64_t ioc_rem(int64_t a, int64_t b) {
    if (b == 0) ioc_panic("division by zero");
    if (b == -1) return 0;
    return a % b;
}
static uint64_t ioc_udiv(uint64_t a, uint64_t b) {
    if (b == 0) ioc_panic("division by zero");
    return a / b;
}
static uint64_t ioc_urem(uint64_t a, uint64_t b) {
    if (b == 0) ioc_panic("division by zero");
    return a % b;
}

/* ---- strings (v0.0.5: allocated strings are released when the program exits) */
static ioc_string ioc_cat(int n, ioc_string *parts) {
    size_t total = 0;
    for (int i = 0; i < n; i++) total += strlen(parts[i]);
    char *out = (char *)ioc_alloc(total + 1);
    size_t at = 0;
    for (int i = 0; i < n; i++) {
        size_t l = strlen(parts[i]);
        memcpy(out + at, parts[i], l);
        at += l;
    }
    out[at] = 0;
    return out;
}
static ioc_string ioc_str_i64(int64_t v) {
    char *b = (char *)ioc_alloc(24);
    snprintf(b, 24, "%lld", (long long)v);
    return b;
}
static ioc_string ioc_str_u64(uint64_t v) {
    char *b = (char *)ioc_alloc(24);
    snprintf(b, 24, "%llu", (unsigned long long)v);
    return b;
}
static ioc_string ioc_str_f64(double v) {
    char *b = (char *)ioc_alloc(40);
    snprintf(b, 40, "%.15g", v);
    if (strtod(b, NULL) != v) snprintf(b, 40, "%.17g", v);
    return b;
}
static ioc_string ioc_str_bool(bool v) { return v ? "true" : "false"; }
static ioc_string ioc_str_char(uint32_t c) {
    char *b = (char *)ioc_alloc(5);
    if (c < 0x80) { b[0] = (char)c; b[1] = 0; }
    else if (c < 0x800) { b[0] = (char)(0xC0 | (c >> 6)); b[1] = (char)(0x80 | (c & 0x3F)); b[2] = 0; }
    else if (c < 0x10000) { b[0] = (char)(0xE0 | (c >> 12)); b[1] = (char)(0x80 | ((c >> 6) & 0x3F)); b[2] = (char)(0x80 | (c & 0x3F)); b[3] = 0; }
    else { b[0] = (char)(0xF0 | (c >> 18)); b[1] = (char)(0x80 | ((c >> 12) & 0x3F)); b[2] = (char)(0x80 | ((c >> 6) & 0x3F)); b[3] = (char)(0x80 | (c & 0x3F)); b[4] = 0; }
    return b;
}
static bool ioc_streq(ioc_string a, ioc_string b) { return strcmp(a, b) == 0; }
static int ioc_strcmp(ioc_string a, ioc_string b) { return strcmp(a, b); }
static int32_t ioc_strlen(ioc_string s) {
    /* length in characters (UTF-8 aware) */
    int32_t n = 0;
    for (const unsigned char *p = (const unsigned char *)s; *p; p++) if ((*p & 0xC0) != 0x80) n++;
    return n;
}
static bool ioc_contains(ioc_string s, ioc_string t) { return strstr(s, t) != NULL; }
static bool ioc_starts(ioc_string s, ioc_string t) { return strncmp(s, t, strlen(t)) == 0; }
static bool ioc_ends(ioc_string s, ioc_string t) {
    size_t a = strlen(s), b = strlen(t);
    return b <= a && memcmp(s + a - b, t, b) == 0;
}
static ioc_string ioc_upper(ioc_string s) {
    size_t n = strlen(s);
    char *o = (char *)ioc_alloc(n + 1);
    for (size_t i = 0; i <= n; i++) o[i] = (char)toupper((unsigned char)s[i]);
    return o;
}
static ioc_string ioc_lower(ioc_string s) {
    size_t n = strlen(s);
    char *o = (char *)ioc_alloc(n + 1);
    for (size_t i = 0; i <= n; i++) o[i] = (char)tolower((unsigned char)s[i]);
    return o;
}
static ioc_string ioc_trim(ioc_string s) {
    while (*s && isspace((unsigned char)*s)) s++;
    size_t n = strlen(s);
    while (n > 0 && isspace((unsigned char)s[n - 1])) n--;
    char *o = (char *)ioc_alloc(n + 1);
    memcpy(o, s, n);
    o[n] = 0;
    return o;
}
static ioc_string ioc_substring(ioc_string s, int32_t start, int32_t len) {
    /* byte based for now; fine for ASCII */
    int32_t n = (int32_t)strlen(s);
    if (start < 0 || len < 0 || start > n || start + len > n) ioc_panic("substring out of range");
    char *o = (char *)ioc_alloc((size_t)len + 1);
    memcpy(o, s + start, (size_t)len);
    o[len] = 0;
    return o;
}

/* ---- console */
static void ioc_println(ioc_string s) { fputs(s, stdout); fputc('\n', stdout); }
static void ioc_print(ioc_string s) { fputs(s, stdout); }
static void ioc_eprintln(ioc_string s) { fflush(stdout); fputs(s, stderr); fputc('\n', stderr); }

/* ---- math */
static int64_t ioc_abs_i(int64_t v) { return v < 0 ? (int64_t)(0 - (uint64_t)v) : v; }
static int64_t ioc_min_i(int64_t a, int64_t b) { return a < b ? a : b; }
static int64_t ioc_max_i(int64_t a, int64_t b) { return a > b ? a : b; }
static double ioc_min_d(double a, double b) { return a < b ? a : b; }
static double ioc_max_d(double a, double b) { return a > b ? a : b; }
static ioc_string ioc_empty_string(void) { return ""; }
static int64_t ioc_check_index(int64_t i, int64_t len) {
    if (i < 0 || i >= len) ioc_panic("index out of range");
    return i;
}
#define IOC_INDEX(i, len) ioc_check_index((int64_t)(i), (int64_t)(len))
/* ---- objects (v0.0.7): every class value is an owned pointer; moving out leaves NULL, drop(NULL) does nothing */
static void *ioc_zalloc(size_t n) {
    void *p = calloc(1, n ? n : 1);
    if (!p) ioc_panic("out of memory");
    return p;
}
#define IOC_TAKE(p) ({ __typeof__(p) ioc_t_ = (p); (p) = NULL; ioc_t_; })

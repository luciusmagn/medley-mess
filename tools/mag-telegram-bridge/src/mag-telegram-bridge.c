#define _POSIX_C_SOURCE 200809L

#include <ctype.h>
#include <dlfcn.h>
#include <errno.h>
#include <inttypes.h>
#include <signal.h>
#include <stdarg.h>
#include <stdbool.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <sys/types.h>
#include <time.h>
#include <unistd.h>

#define MAG_TG_REQUEST_PATH "/tmp/mag-telegram-request"
#define MAG_TG_RESPONSE_PATH "/tmp/mag-telegram-response"
#define MAG_TG_PID_PATH "/tmp/mag-telegram-bridge.pid"
#define MAG_TG_DEFAULT_CONFIG_REL ".config/mag-telegram/config"
#define MAG_TG_MAX_TEXT 4096
#define MAG_TG_MAX_LINE 1024
#define MAG_TG_MAX_CHATS 256
#define MAG_TG_MAX_USERS 512
#define MAG_TG_MAX_MESSAGES 1024
#define MAG_TG_HISTORY_LIMIT 40
#define MAG_TG_HISTORY_WAIT_MS 1500

struct TdApi {
  void *handle;
  void *client;
  void *(*create)(void);
  void (*send)(void *, const char *);
  const char *(*receive)(void *, double);
  const char *(*execute)(void *, const char *);
  void (*destroy)(void *);
};

struct Chat {
  long long id;
  char title[256];
  char kind[48];
  long long order;
};

struct Message {
  long long chat_id;
  long long message_id;
  long long sender_id;
  bool outgoing;
  char text[512];
};

struct User {
  long long id;
  char name[256];
};

struct Bridge {
  bool live;
  bool stop;
  char backend[32];
  char auth_state[96];
  char last_error[512];
  char data_dir[512];
  char files_dir[512];
  char config_path[512];
  char tdlib_library[512];
  char api_id[64];
  char api_hash[256];
  char encryption_key[256];
  struct TdApi td;
  struct Chat chats[MAG_TG_MAX_CHATS];
  size_t chat_count;
  struct User users[MAG_TG_MAX_USERS];
  size_t user_count;
  struct Message messages[MAG_TG_MAX_MESSAGES];
  size_t message_count;
};

static volatile sig_atomic_t g_stop = 0;

static void on_signal(int signo) {
  (void)signo;
  g_stop = 1;
}

static const char *env_or(const char *name, const char *fallback) {
  const char *value = getenv(name);
  return (value && value[0]) ? value : fallback;
}

static void copy_str(char *dst, size_t cap, const char *src) {
  if (cap == 0) return;
  if (!src) src = "";
  snprintf(dst, cap, "%s", src);
}

static void appendf(char *dst, size_t cap, const char *fmt, ...) {
  size_t len;
  va_list ap;
  if (cap == 0) return;
  len = strlen(dst);
  if (len >= cap - 1) return;
  va_start(ap, fmt);
  vsnprintf(dst + len, cap - len, fmt, ap);
  va_end(ap);
}

static char *read_file(const char *path) {
  FILE *f = fopen(path, "rb");
  long len;
  char *buf;
  if (!f) return NULL;
  if (fseek(f, 0, SEEK_END) != 0) {
    fclose(f);
    return NULL;
  }
  len = ftell(f);
  if (len < 0) {
    fclose(f);
    return NULL;
  }
  rewind(f);
  buf = calloc((size_t)len + 1, 1);
  if (!buf) {
    fclose(f);
    return NULL;
  }
  if (len > 0 && fread(buf, 1, (size_t)len, f) != (size_t)len) {
    free(buf);
    fclose(f);
    return NULL;
  }
  fclose(f);
  return buf;
}

static bool write_file_atomic(const char *path, const char *text) {
  char tmp[512];
  FILE *f;
  snprintf(tmp, sizeof(tmp), "%s.%ld.tmp", path, (long)getpid());
  f = fopen(tmp, "wb");
  if (!f) return false;
  if (text && fputs(text, f) == EOF) {
    fclose(f);
    unlink(tmp);
    return false;
  }
  if (fclose(f) != 0) {
    unlink(tmp);
    return false;
  }
  return rename(tmp, path) == 0;
}

static bool process_alive(long pid) {
  if (pid <= 0) return false;
  if (kill((pid_t)pid, 0) == 0) return true;
  return errno == EPERM;
}

static bool daemon_already_running(void) {
  char *text = read_file(MAG_TG_PID_PATH);
  long pid;
  if (!text) return false;
  pid = strtol(text, NULL, 10);
  free(text);
  if (process_alive(pid)) return true;
  unlink(MAG_TG_PID_PATH);
  return false;
}

static void write_pid_file(void) {
  char text[64];
  snprintf(text, sizeof(text), "%ld\n", (long)getpid());
  write_file_atomic(MAG_TG_PID_PATH, text);
}

static void trim_right(char *s) {
  size_t n;
  if (!s) return;
  n = strlen(s);
  while (n > 0 && (s[n - 1] == '\n' || s[n - 1] == '\r' || s[n - 1] == ' ' || s[n - 1] == '\t')) {
    s[--n] = 0;
  }
}

static const char *skip_space(const char *p) {
  while (*p && isspace((unsigned char)*p)) p++;
  return p;
}

static char *skip_space_mut(char *p) {
  while (*p && isspace((unsigned char)*p)) p++;
  return p;
}

static void trim_both(char **p) {
  *p = skip_space_mut(*p);
  trim_right(*p);
}

static void unquote_value(char *s) {
  size_t len;
  if (!s) return;
  len = strlen(s);
  if (len >= 2 && ((s[0] == '"' && s[len - 1] == '"') || (s[0] == '\'' && s[len - 1] == '\''))) {
    memmove(s, s + 1, len - 2);
    s[len - 2] = 0;
  }
}

static void sleep_ms(long ms) {
  struct timespec req;
  req.tv_sec = ms / 1000;
  req.tv_nsec = (ms % 1000) * 1000000L;
  while (nanosleep(&req, &req) != 0 && errno == EINTR) {
  }
}

static void json_escape(const char *src, char *dst, size_t cap) {
  size_t used = 0;
  if (cap == 0) return;
  dst[0] = 0;
  if (!src) return;
  for (; *src && used + 1 < cap; src++) {
    unsigned char ch = (unsigned char)*src;
    const char *rep = NULL;
    char hex[7];
    if (ch == '"') rep = "\\\"";
    else if (ch == '\\') rep = "\\\\";
    else if (ch == '\n') rep = "\\n";
    else if (ch == '\r') rep = "\\r";
    else if (ch == '\t') rep = "\\t";
    else if (ch < 32) {
      snprintf(hex, sizeof(hex), "\\u%04x", ch);
      rep = hex;
    }
    if (rep) {
      size_t rlen = strlen(rep);
      if (used + rlen >= cap) break;
      memcpy(dst + used, rep, rlen);
      used += rlen;
    } else {
      dst[used++] = (char)ch;
    }
  }
  dst[used] = 0;
}

static bool json_extract_string(const char *json, const char *key, char *out, size_t cap) {
  char needle[128];
  const char *p;
  size_t n = 0;
  snprintf(needle, sizeof(needle), "\"%s\"", key);
  p = strstr(json, needle);
  if (!p) return false;
  p = strchr(p + strlen(needle), ':');
  if (!p) return false;
  p = skip_space(p + 1);
  if (*p != '"') return false;
  p++;
  while (*p && *p != '"' && n + 1 < cap) {
    if (*p == '\\' && p[1]) {
      p++;
      switch (*p) {
        case 'n': out[n++] = '\n'; break;
        case 'r': out[n++] = '\r'; break;
        case 't': out[n++] = '\t'; break;
        case '"': out[n++] = '"'; break;
        case '\\': out[n++] = '\\'; break;
        default: out[n++] = *p; break;
      }
    } else {
      out[n++] = *p;
    }
    p++;
  }
  out[n] = 0;
  return true;
}

static bool json_extract_string_after(const char *json, const char *marker, const char *key,
                                      char *out, size_t cap) {
  const char *p = strstr(json, marker);
  return p ? json_extract_string(p, key, out, cap) : false;
}

static bool json_extract_i64(const char *json, const char *key, long long *out) {
  char needle[128];
  const char *p;
  char *end = NULL;
  snprintf(needle, sizeof(needle), "\"%s\"", key);
  p = strstr(json, needle);
  if (!p) return false;
  p = strchr(p + strlen(needle), ':');
  if (!p) return false;
  p = skip_space(p + 1);
  if (*p == '"') p++;
  errno = 0;
  *out = strtoll(p, &end, 10);
  return errno == 0 && end && end != p;
}

static bool json_extract_i64_after(const char *json, const char *marker, const char *key, long long *out) {
  const char *p = strstr(json, marker);
  return p ? json_extract_i64(p, key, out) : false;
}

static bool json_extract_bool(const char *json, const char *key, bool *out) {
  char needle[128];
  const char *p;
  snprintf(needle, sizeof(needle), "\"%s\"", key);
  p = strstr(json, needle);
  if (!p) return false;
  p = strchr(p + strlen(needle), ':');
  if (!p) return false;
  p = skip_space(p + 1);
  if (strncmp(p, "true", 4) == 0) {
    *out = true;
    return true;
  }
  if (strncmp(p, "false", 5) == 0) {
    *out = false;
    return true;
  }
  return false;
}

static bool json_extract_bool_after(const char *json, const char *marker, const char *key, bool *out) {
  const char *p = strstr(json, marker);
  return p ? json_extract_bool(p, key, out) : false;
}

static const char *json_object_end(const char *p) {
  int depth = 0;
  bool in_string = false;
  bool escaped = false;
  for (; *p; p++) {
    if (in_string) {
      if (escaped) escaped = false;
      else if (*p == '\\') escaped = true;
      else if (*p == '"') in_string = false;
      continue;
    }
    if (*p == '"') in_string = true;
    else if (*p == '{') depth++;
    else if (*p == '}') {
      if (depth > 0 && --depth == 0) return p + 1;
    }
  }
  return NULL;
}

static const char *chat_kind_from_json(const char *json) {
  bool is_channel = false;
  if (strstr(json, "chatTypePrivate")) return "private";
  if (strstr(json, "chatTypeBasicGroup")) return "group";
  if (strstr(json, "chatTypeSupergroup")) {
    if (json_extract_bool_after(json, "chatTypeSupergroup", "is_channel", &is_channel) && is_channel) return "channel";
    return "supergroup";
  }
  if (strstr(json, "chatTypeSecret")) return "secret";
  return "chat";
}

static struct Chat *lookup_chat(struct Bridge *b, long long id) {
  for (size_t i = 0; i < b->chat_count; i++) {
    if (b->chats[i].id == id) return &b->chats[i];
  }
  return NULL;
}

static struct Chat *find_chat(struct Bridge *b, long long id) {
  struct Chat *chat = lookup_chat(b, id);
  if (chat) return chat;
  if (b->chat_count >= MAG_TG_MAX_CHATS) return NULL;
  b->chats[b->chat_count].id = id;
  b->chats[b->chat_count].title[0] = 0;
  b->chats[b->chat_count].kind[0] = 0;
  b->chats[b->chat_count].order = 0;
  return &b->chats[b->chat_count++];
}

static struct User *find_user(struct Bridge *b, long long id, bool create) {
  for (size_t i = 0; i < b->user_count; i++) {
    if (b->users[i].id == id) return &b->users[i];
  }
  if (!create || b->user_count >= MAG_TG_MAX_USERS) return NULL;
  b->users[b->user_count].id = id;
  b->users[b->user_count].name[0] = 0;
  return &b->users[b->user_count++];
}

static void set_user_name(struct Bridge *b, long long id, const char *name) {
  struct User *user;
  if (id == 0) return;
  user = find_user(b, id, true);
  if (!user) return;
  copy_str(user->name, sizeof(user->name), name && name[0] ? name : "unknown");
}

static bool message_text_from_json(const char *json, char *text, size_t cap) {
  char caption[512];
  if (strstr(json, "messageText") &&
      json_extract_string_after(json, "formattedText", "text", text, cap)) {
    return true;
  }
  if (json_extract_string_after(json, "\"caption\"", "text", caption, sizeof(caption)) && caption[0]) {
    copy_str(text, cap, "[caption] ");
    appendf(text, cap, "%s", caption);
    return true;
  }
  copy_str(text, cap, "[non-text message]");
  return false;
}

static void add_message(struct Bridge *b, long long chat_id, long long message_id, long long sender_id,
                        bool outgoing, const char *text) {
  struct Message *m;
  for (size_t i = 0; i < b->message_count; i++) {
    m = &b->messages[i];
    if (m->chat_id == chat_id && m->message_id == message_id) {
      m->sender_id = sender_id;
      m->outgoing = outgoing;
      copy_str(m->text, sizeof(m->text), text && text[0] ? text : "[non-text message]");
      return;
    }
  }
  if (b->message_count >= MAG_TG_MAX_MESSAGES) {
    memmove(&b->messages[0], &b->messages[1], sizeof(b->messages[0]) * (MAG_TG_MAX_MESSAGES - 1));
    b->message_count = MAG_TG_MAX_MESSAGES - 1;
  }
  m = &b->messages[b->message_count++];
  m->chat_id = chat_id;
  m->message_id = message_id;
  m->sender_id = sender_id;
  m->outgoing = outgoing;
  copy_str(m->text, sizeof(m->text), text && text[0] ? text : "[non-text message]");
}

static int message_ptr_compare_asc(const void *ap, const void *bp) {
  const struct Message *a = *(const struct Message * const *)ap;
  const struct Message *b = *(const struct Message * const *)bp;
  if (a->message_id < b->message_id) return -1;
  if (a->message_id > b->message_id) return 1;
  if (a->sender_id < b->sender_id) return -1;
  if (a->sender_id > b->sender_id) return 1;
  return 0;
}

static int chat_ptr_compare_desc(const void *ap, const void *bp) {
  const struct Chat *a = *(const struct Chat * const *)ap;
  const struct Chat *b = *(const struct Chat * const *)bp;
  if (a->order > b->order) return -1;
  if (a->order < b->order) return 1;
  if (a->id > b->id) return -1;
  if (a->id < b->id) return 1;
  return 0;
}

static void init_mock(struct Bridge *b) {
  struct Chat *c;
  copy_str(b->backend, sizeof(b->backend), "mock");
  copy_str(b->auth_state, sizeof(b->auth_state), "mock-ready");
  c = find_chat(b, 1001);
  copy_str(c->title, sizeof(c->title), "Saved Messages");
  copy_str(c->kind, sizeof(c->kind), "private");
  c->order = 300;
  c = find_chat(b, 1002);
  copy_str(c->title, sizeof(c->title), "Lisp Notes Group");
  copy_str(c->kind, sizeof(c->kind), "group");
  c->order = 200;
  c = find_chat(b, 1003);
  copy_str(c->title, sizeof(c->title), "Systems Channel");
  copy_str(c->kind, sizeof(c->kind), "channel");
  c->order = 100;
  set_user_name(b, 42, "Mock Group Friend");
  set_user_name(b, 77, "Mock Channel Admin");
  add_message(b, 1001, 1, 0, false, "Mock bridge is running. Install tdlib and set API credentials for live Telegram.");
  add_message(b, 1002, 1, 42, false, "This is group text; no images/reactions are needed for the first Medley client.");
  add_message(b, 1003, 1, 77, false, "Channel post text will render through the same message path.");
}

static bool ensure_dir(const char *path) {
  char tmp[512];
  size_t len;
  copy_str(tmp, sizeof(tmp), path);
  len = strlen(tmp);
  if (len == 0) return false;
  for (char *p = tmp + 1; *p; p++) {
    if (*p == '/') {
      *p = 0;
      mkdir(tmp, 0700);
      *p = '/';
    }
  }
  return mkdir(tmp, 0700) == 0 || errno == EEXIST;
}

static void apply_config_value(struct Bridge *b, const char *key, const char *value) {
  if (!key || !value) return;
  if (strcmp(key, "api_id") == 0 || strcmp(key, "MAG_TELEGRAM_API_ID") == 0) {
    copy_str(b->api_id, sizeof(b->api_id), value);
  } else if (strcmp(key, "api_hash") == 0 || strcmp(key, "MAG_TELEGRAM_API_HASH") == 0) {
    copy_str(b->api_hash, sizeof(b->api_hash), value);
  } else if (strcmp(key, "encryption_key") == 0 || strcmp(key, "MAG_TELEGRAM_ENCRYPTION_KEY") == 0) {
    copy_str(b->encryption_key, sizeof(b->encryption_key), value);
  } else if (strcmp(key, "data_dir") == 0) {
    copy_str(b->data_dir, sizeof(b->data_dir), value);
  } else if (strcmp(key, "data_base_dir") == 0 || strcmp(key, "MAG_TELEGRAM_DATA_DIR") == 0) {
    snprintf(b->data_dir, sizeof(b->data_dir), "%s/tdlib", value);
  } else if (strcmp(key, "tdlib_library") == 0 || strcmp(key, "MAG_TELEGRAM_TDLIB_LIBRARY") == 0) {
    copy_str(b->tdlib_library, sizeof(b->tdlib_library), value);
  }
}

static void read_config_file(struct Bridge *b) {
  const char *home = env_or("HOME", "/tmp");
  const char *path = getenv("MAG_TELEGRAM_CONFIG");
  FILE *f;
  char line[MAG_TG_MAX_LINE];
  if (path && path[0]) copy_str(b->config_path, sizeof(b->config_path), path);
  else snprintf(b->config_path, sizeof(b->config_path), "%s/%s", home, MAG_TG_DEFAULT_CONFIG_REL);
  f = fopen(b->config_path, "r");
  if (!f) return;
  while (fgets(line, sizeof(line), f)) {
    char *p = line;
    char *eq;
    char *key;
    char *value;
    trim_both(&p);
    if (p[0] == 0 || p[0] == '#' || p[0] == ';') continue;
    eq = strchr(p, '=');
    if (!eq) continue;
    *eq = 0;
    key = p;
    value = eq + 1;
    trim_both(&key);
    trim_both(&value);
    unquote_value(value);
    apply_config_value(b, key, value);
  }
  fclose(f);
}

static void apply_env_overrides(struct Bridge *b) {
  const char *value;
  if ((value = getenv("MAG_TELEGRAM_API_ID")) && value[0]) copy_str(b->api_id, sizeof(b->api_id), value);
  if ((value = getenv("MAG_TELEGRAM_API_HASH")) && value[0]) copy_str(b->api_hash, sizeof(b->api_hash), value);
  if ((value = getenv("MAG_TELEGRAM_ENCRYPTION_KEY")) && value[0]) {
    copy_str(b->encryption_key, sizeof(b->encryption_key), value);
  }
  if ((value = getenv("MAG_TELEGRAM_DATA_DIR")) && value[0]) {
    snprintf(b->data_dir, sizeof(b->data_dir), "%s/tdlib", value);
  }
  if ((value = getenv("MAG_TELEGRAM_TDLIB_LIBRARY")) && value[0]) {
    copy_str(b->tdlib_library, sizeof(b->tdlib_library), value);
  }
}

static void load_runtime_config(struct Bridge *b) {
  const char *home = env_or("HOME", "/tmp");
  read_config_file(b);
  apply_env_overrides(b);
  if (b->encryption_key[0] == 0) copy_str(b->encryption_key, sizeof(b->encryption_key), "mag-telegram-local");
  if (b->data_dir[0] == 0) snprintf(b->data_dir, sizeof(b->data_dir), "%s/.local/share/mag-telegram/tdlib", home);
}

static bool check_tdlib_library(struct Bridge *b) {
  const char *lib = b->tdlib_library[0] ? b->tdlib_library : "libtdjson.so";
  void *handle = dlopen(lib, RTLD_NOW | RTLD_LOCAL);
  if (!handle) {
    b->last_error[0] = 0;
    appendf(b->last_error, sizeof(b->last_error), "dlopen failed for ");
    appendf(b->last_error, sizeof(b->last_error), "%s", lib);
    appendf(b->last_error, sizeof(b->last_error), ": ");
    appendf(b->last_error, sizeof(b->last_error), "%s", dlerror());
    return false;
  }
  if (!dlsym(handle, "td_json_client_create") ||
      !dlsym(handle, "td_json_client_send") ||
      !dlsym(handle, "td_json_client_receive") ||
      !dlsym(handle, "td_json_client_destroy")) {
    copy_str(b->last_error, sizeof(b->last_error), "libtdjson is missing required JSON client symbols");
    dlclose(handle);
    return false;
  }
  dlclose(handle);
  return true;
}

static bool load_tdlib(struct Bridge *b) {
  const char *lib = b->tdlib_library[0] ? b->tdlib_library : "libtdjson.so";
  b->td.handle = dlopen(lib, RTLD_NOW | RTLD_LOCAL);
  if (!b->td.handle) {
    b->last_error[0] = 0;
    appendf(b->last_error, sizeof(b->last_error), "dlopen failed for ");
    appendf(b->last_error, sizeof(b->last_error), "%s", lib);
    appendf(b->last_error, sizeof(b->last_error), ": ");
    appendf(b->last_error, sizeof(b->last_error), "%s", dlerror());
    return false;
  }
  b->td.create = dlsym(b->td.handle, "td_json_client_create");
  b->td.send = dlsym(b->td.handle, "td_json_client_send");
  b->td.receive = dlsym(b->td.handle, "td_json_client_receive");
  b->td.execute = dlsym(b->td.handle, "td_json_client_execute");
  b->td.destroy = dlsym(b->td.handle, "td_json_client_destroy");
  if (!b->td.create || !b->td.send || !b->td.receive || !b->td.destroy) {
    snprintf(b->last_error, sizeof(b->last_error), "libtdjson is missing required JSON client symbols");
    dlclose(b->td.handle);
    memset(&b->td, 0, sizeof(b->td));
    return false;
  }
  b->td.client = b->td.create();
  if (!b->td.client) {
    snprintf(b->last_error, sizeof(b->last_error), "td_json_client_create returned NULL");
    dlclose(b->td.handle);
    memset(&b->td, 0, sizeof(b->td));
    return false;
  }
  return true;
}

static void td_send_json(struct Bridge *b, const char *json) {
  if (b->live && b->td.send && b->td.client) b->td.send(b->td.client, json);
}

static void td_request_chat_history(struct Bridge *b, long long chat_id, long long from_message_id) {
  char req[512];
  if (!b->live || strcmp(b->auth_state, "ready") != 0) return;
  snprintf(req, sizeof(req),
           "{\"@type\":\"getChatHistory\",\"chat_id\":%lld,"
           "\"from_message_id\":%lld,\"offset\":0,\"limit\":%d,"
           "\"only_local\":false,\"@extra\":\"mag-history:%lld\"}",
           chat_id, from_message_id, MAG_TG_HISTORY_LIMIT, chat_id);
  td_send_json(b, req);
}

static void td_request_user(struct Bridge *b, long long user_id) {
  char req[256];
  if (!b->live || strcmp(b->auth_state, "ready") != 0 || user_id == 0) return;
  if (find_user(b, user_id, false)) return;
  snprintf(req, sizeof(req), "{\"@type\":\"getUser\",\"user_id\":%lld,\"@extra\":\"mag-user:%lld\"}",
           user_id, user_id);
  td_send_json(b, req);
}

static void send_tdlib_parameters(struct Bridge *b) {
  char data[1024], files[1024], hash[512], key[512], req[4096];
  json_escape(b->data_dir, data, sizeof(data));
  json_escape(b->files_dir, files, sizeof(files));
  json_escape(b->api_hash, hash, sizeof(hash));
  json_escape(b->encryption_key, key, sizeof(key));
  snprintf(req, sizeof(req),
           "{\"@type\":\"setTdlibParameters\","
           "\"use_test_dc\":false,"
           "\"database_directory\":\"%s\","
           "\"files_directory\":\"%s\","
           "\"database_encryption_key\":\"%s\","
           "\"use_file_database\":false,"
           "\"use_chat_info_database\":true,"
           "\"use_message_database\":true,"
           "\"use_secret_chats\":false,"
           "\"api_id\":%s,"
           "\"api_hash\":\"%s\","
           "\"system_language_code\":\"en\","
           "\"device_model\":\"Medley Interlisp\","
           "\"system_version\":\"Guix\","
           "\"application_version\":\"0.1\"}",
           data, files, key, b->api_id, hash);
  td_send_json(b, req);
}

static bool init_live(struct Bridge *b) {
  load_runtime_config(b);
  if (b->api_id[0] == 0 || b->api_hash[0] == 0) {
    b->last_error[0] = 0;
    appendf(b->last_error, sizeof(b->last_error), "Telegram api_id/api_hash are not set; use env or ");
    appendf(b->last_error, sizeof(b->last_error), "%s",
            b->config_path[0] ? b->config_path : "~/.config/mag-telegram/config");
    return false;
  }
  copy_str(b->files_dir, sizeof(b->files_dir), b->data_dir);
  appendf(b->files_dir, sizeof(b->files_dir), "/files");
  ensure_dir(b->files_dir);
  if (!load_tdlib(b)) return false;
  b->live = true;
  copy_str(b->backend, sizeof(b->backend), "tdlib");
  copy_str(b->auth_state, sizeof(b->auth_state), "starting");
  if (b->td.execute) b->td.execute(b->td.client, "{\"@type\":\"setLogVerbosityLevel\",\"new_verbosity_level\":1}");
  td_send_json(b, "{\"@type\":\"getOption\",\"name\":\"version\",\"@extra\":\"version\"}");
  return true;
}

static void handle_auth_update(struct Bridge *b, const char *json) {
  if (strstr(json, "authorizationStateWaitTdlibParameters")) {
    copy_str(b->auth_state, sizeof(b->auth_state), "wait-tdlib-parameters");
    send_tdlib_parameters(b);
  } else if (strstr(json, "authorizationStateWaitPhoneNumber")) {
    copy_str(b->auth_state, sizeof(b->auth_state), "wait-phone");
  } else if (strstr(json, "authorizationStateWaitCode")) {
    copy_str(b->auth_state, sizeof(b->auth_state), "wait-code");
  } else if (strstr(json, "authorizationStateWaitPassword")) {
    copy_str(b->auth_state, sizeof(b->auth_state), "wait-password");
  } else if (strstr(json, "authorizationStateWaitRegistration")) {
    copy_str(b->auth_state, sizeof(b->auth_state), "wait-registration");
  } else if (strstr(json, "authorizationStateWaitOtherDeviceConfirmation")) {
    copy_str(b->auth_state, sizeof(b->auth_state), "wait-other-device");
  } else if (strstr(json, "authorizationStateWaitEmailAddress")) {
    copy_str(b->auth_state, sizeof(b->auth_state), "wait-email-address");
  } else if (strstr(json, "authorizationStateWaitEmailCode")) {
    copy_str(b->auth_state, sizeof(b->auth_state), "wait-email-code");
  } else if (strstr(json, "authorizationStateReady")) {
    copy_str(b->auth_state, sizeof(b->auth_state), "ready");
    td_send_json(b, "{\"@type\":\"loadChats\",\"chat_list\":{\"@type\":\"chatListMain\"},\"limit\":100}");
  } else if (strstr(json, "authorizationStateClosing")) {
    copy_str(b->auth_state, sizeof(b->auth_state), "closing");
  } else if (strstr(json, "authorizationStateClosed")) {
    copy_str(b->auth_state, sizeof(b->auth_state), "closed");
    b->stop = true;
  }
}

static void handle_error_response(struct Bridge *b, const char *json) {
  long long code = 0;
  char message[256] = "";
  if (!strstr(json, "\"@type\":\"error\"")) return;
  json_extract_i64(json, "code", &code);
  json_extract_string(json, "message", message, sizeof(message));
  snprintf(b->last_error, sizeof(b->last_error), "tdlib error %lld: %s",
           code, message[0] ? message : "unknown");
}

static void handle_chat_update(struct Bridge *b, const char *json) {
  long long id = 0;
  long long order = 0;
  char title[256];
  struct Chat *chat;
  if (!strstr(json, "updateNewChat") &&
      !strstr(json, "updateChatTitle") &&
      !strstr(json, "updateChatPosition") &&
      !strstr(json, "updateChatLastMessage") &&
      !strstr(json, "updateChatDraftMessage")) {
    return;
  }
  if (!json_extract_i64(json, "chat_id", &id) && !json_extract_i64(json, "id", &id)) return;
  chat = find_chat(b, id);
  if (!chat) return;
  if (json_extract_string(json, "title", title, sizeof(title))) copy_str(chat->title, sizeof(chat->title), title);
  if (json_extract_i64_after(json, "chatListMain", "order", &order) ||
      json_extract_i64(json, "order", &order)) {
    chat->order = order;
  }
  if (chat->kind[0] == 0) copy_str(chat->kind, sizeof(chat->kind), chat_kind_from_json(json));
}

static void handle_user_update(struct Bridge *b, const char *json) {
  long long id = 0;
  char first[128] = "";
  char last[128] = "";
  char username[128] = "";
  char name[256];
  if (!strstr(json, "updateUser") && !strstr(json, "\"@type\":\"user\"")) return;
  if (!json_extract_i64(json, "id", &id)) return;
  json_extract_string(json, "first_name", first, sizeof(first));
  json_extract_string(json, "last_name", last, sizeof(last));
  json_extract_string(json, "username", username, sizeof(username));
  if (first[0] && last[0]) snprintf(name, sizeof(name), "%s %s", first, last);
  else if (first[0]) copy_str(name, sizeof(name), first);
  else if (last[0]) copy_str(name, sizeof(name), last);
  else if (username[0]) snprintf(name, sizeof(name), "@%s", username);
  else snprintf(name, sizeof(name), "user %lld", id);
  set_user_name(b, id, name);
}

static void cache_message_object(struct Bridge *b, const char *json) {
  long long chat_id = 0, id = 0, sender_id = 0;
  char text[512];
  bool outgoing = strstr(json, "\"is_outgoing\":true") != NULL;
  if (!json_extract_i64(json, "chat_id", &chat_id)) return;
  if (!json_extract_i64(json, "id", &id)) id = (long long)time(NULL);
  if (!json_extract_i64_after(json, "messageSenderUser", "user_id", &sender_id)) {
    json_extract_i64_after(json, "messageSenderChat", "chat_id", &sender_id);
  }
  if (sender_id != 0 && strstr(json, "messageSenderUser")) td_request_user(b, sender_id);
  message_text_from_json(json, text, sizeof(text));
  add_message(b, chat_id, id, sender_id, outgoing, text);
}

static void handle_message_update(struct Bridge *b, const char *json) {
  const char *message;
  if (!strstr(json, "updateNewMessage") && !strstr(json, "updateMessageSendSucceeded")) return;
  message = strstr(json, "\"@type\":\"message\"");
  cache_message_object(b, message ? message : json);
}

static void handle_messages_response(struct Bridge *b, const char *json) {
  const char *p = json;
  while ((p = strstr(p, "\"@type\":\"message\"")) != NULL) {
    const char *start = p;
    const char *end;
    while (start > json && *start != '{') start--;
    end = (*start == '{') ? json_object_end(start) : NULL;
    if (end && end > start && (size_t)(end - start) < 32768) {
      size_t len = (size_t)(end - start);
      char *one = calloc(len + 1, 1);
      if (one) {
        memcpy(one, start, len);
        cache_message_object(b, one);
        free(one);
      }
      p = end;
    } else {
      cache_message_object(b, p);
      p += strlen("\"@type\":\"message\"");
    }
  }
}

static void handle_td_update(struct Bridge *b, const char *json) {
  if (!json) return;
  handle_error_response(b, json);
  if (strstr(json, "\"@type\":\"messages\"")) handle_messages_response(b, json);
  if (strstr(json, "updateAuthorizationState")) handle_auth_update(b, json);
  handle_user_update(b, json);
  handle_chat_update(b, json);
  handle_message_update(b, json);
}

static void poll_tdlib(struct Bridge *b) {
  const char *result;
  if (!b->live || !b->td.receive) return;
  result = b->td.receive(b->td.client, 0.05);
  if (result) handle_td_update(b, result);
}

static void poll_tdlib_for(struct Bridge *b, long ms) {
  long elapsed = 0;
  while (elapsed < ms) {
    poll_tdlib(b);
    sleep_ms(50);
    elapsed += 50;
  }
}

static const char *auth_action_for_state(struct Bridge *b) {
  if (!b->live && b->last_error[0]) return "configure";
  if (strcmp(b->auth_state, "wait-phone") == 0) return "auth-phone";
  if (strcmp(b->auth_state, "wait-code") == 0) return "auth-code";
  if (strcmp(b->auth_state, "wait-password") == 0) return "auth-password";
  if (strcmp(b->auth_state, "wait-registration") == 0) return "auth-register";
  if (strcmp(b->auth_state, "wait-email-address") == 0) return "unsupported-email-address";
  if (strcmp(b->auth_state, "wait-email-code") == 0) return "unsupported-email-code";
  if (strcmp(b->auth_state, "wait-other-device") == 0) return "unsupported-other-device";
  if (strcmp(b->auth_state, "ready") == 0) return "none";
  if (strcmp(b->auth_state, "mock-ready") == 0) return "none";
  return "wait";
}

static const char *auth_hint_for_state(struct Bridge *b) {
  if (!b->live && b->last_error[0]) return "Add api_id and api_hash to ~/.config/mag-telegram/config, then restart bridge.";
  if (strcmp(b->auth_state, "wait-phone") == 0) return "Use auth-phone +country-number.";
  if (strcmp(b->auth_state, "wait-code") == 0) return "Use auth-code with the login code Telegram sent.";
  if (strcmp(b->auth_state, "wait-password") == 0) return "Use auth-password with the Telegram 2FA password.";
  if (strcmp(b->auth_state, "wait-registration") == 0) return "Use auth-register FIRST LAST to finish new account registration.";
  if (strcmp(b->auth_state, "wait-email-address") == 0) return "TDLib asks for email address; bridge command is not implemented yet.";
  if (strcmp(b->auth_state, "wait-email-code") == 0) return "TDLib asks for email code; bridge command is not implemented yet.";
  if (strcmp(b->auth_state, "wait-other-device") == 0) return "TDLib asks for other-device confirmation; use phone login for now.";
  if (strcmp(b->auth_state, "ready") == 0) return "Authorized; chats and messages are available.";
  if (strcmp(b->auth_state, "mock-ready") == 0) return "Mock mode; configure credentials for live Telegram.";
  return "Wait for TDLib authorization update, then refresh.";
}

static void response_status(struct Bridge *b, char *out, size_t cap) {
  snprintf(out, cap,
           "Mag Telegram bridge\nbackend=%s\nauth=%s\nauth-action=%s\nauth-hint=%s\nlive=%s\nchats=%zu\nmessages=%zu\nconfig=%s\ndata-dir=%s\ntdlib-library=%s\nlast-error=%s\n",
           b->backend, b->auth_state,
           auth_action_for_state(b), auth_hint_for_state(b),
           b->live ? "yes" : "no", b->chat_count, b->message_count,
           b->config_path[0] ? b->config_path : "unset",
           b->data_dir[0] ? b->data_dir : "unset",
           b->tdlib_library[0] ? b->tdlib_library : "libtdjson.so",
           b->last_error[0] ? b->last_error : "none");
}

static void response_auth_status(struct Bridge *b, char *out, size_t cap) {
  snprintf(out, cap,
           "Mag Telegram auth\nstate=%s\naction=%s\nhint=%s\nlast-error=%s\n",
           b->auth_state,
           auth_action_for_state(b),
           auth_hint_for_state(b),
           b->last_error[0] ? b->last_error : "none");
}

static void response_doctor(char *out, size_t cap) {
  struct Bridge b;
  bool has_credentials;
  bool tdlib_ok;
  memset(&b, 0, sizeof(b));
  load_runtime_config(&b);
  has_credentials = b.api_id[0] != 0 && b.api_hash[0] != 0;
  tdlib_ok = check_tdlib_library(&b);
  snprintf(out, cap,
           "Mag Telegram doctor\n"
           "config=%s\n"
           "api-id=%s\n"
           "api-hash=%s\n"
           "encryption-key=%s\n"
           "data-dir=%s\n"
           "tdlib-library=%s\n"
           "tdlib-load=%s\n"
           "status=%s\n"
           "detail=%s\n",
           b.config_path[0] ? b.config_path : "unset",
           b.api_id[0] ? "set" : "missing",
           b.api_hash[0] ? "set" : "missing",
           b.encryption_key[0] ? "set" : "missing",
           b.data_dir[0] ? b.data_dir : "unset",
           b.tdlib_library[0] ? b.tdlib_library : "libtdjson.so",
           tdlib_ok ? "ok" : "fail",
           (has_credentials && tdlib_ok) ? "ready-for-auth" : "not-ready",
           b.last_error[0] ? b.last_error : "none");
}

static void response_chats(struct Bridge *b, char *out, size_t cap) {
  struct Chat *items[MAG_TG_MAX_CHATS];
  snprintf(out, cap, "Mag Telegram chats (%zu)\n", b->chat_count);
  for (size_t i = 0; i < b->chat_count; i++) {
    items[i] = &b->chats[i];
  }
  if (b->chat_count > 1) qsort(items, b->chat_count, sizeof(items[0]), chat_ptr_compare_desc);
  for (size_t i = 0; i < b->chat_count; i++) {
    struct Chat *chat = items[i];
    appendf(out, cap, "%lld [%s] %s\n", chat->id,
            chat->kind[0] ? chat->kind : "chat",
            chat->title[0] ? chat->title : "(untitled)");
  }
}

static const char *sender_label(struct Bridge *b, struct Message *m, char *buf, size_t cap) {
  struct User *user;
  struct Chat *chat;
  if (m->outgoing) return "me";
  user = find_user(b, m->sender_id, false);
  if (user && user->name[0]) return user->name;
  chat = lookup_chat(b, m->sender_id);
  if (chat && chat->title[0]) return chat->title;
  snprintf(buf, cap, "%lld", m->sender_id);
  return buf;
}

static void build_send_message_request(long long chat_id, const char *text, char *req, size_t cap) {
  char escaped[MAG_TG_MAX_TEXT];
  json_escape(text, escaped, sizeof(escaped));
  snprintf(req, cap,
           "{\"@type\":\"sendMessage\",\"chat_id\":%lld,"
           "\"input_message_content\":{\"@type\":\"inputMessageText\","
           "\"text\":{\"@type\":\"formattedText\",\"text\":\"%s\",\"entities\":[]}}}",
           chat_id, escaped);
}

static void response_messages(struct Bridge *b, long long chat_id, long long from_message_id, char *out, size_t cap) {
  const char *page = from_message_id > 0 ? "older" : "latest";
  struct Message *items[MAG_TG_MAX_MESSAGES];
  size_t total = 0;
  size_t end = 0;
  size_t start = 0;
  size_t count = 0;
  long long oldest_id = 0;
  long long newest_id = 0;
  if (b->live && strcmp(b->auth_state, "ready") == 0) {
    td_request_chat_history(b, chat_id, from_message_id);
    poll_tdlib_for(b, MAG_TG_HISTORY_WAIT_MS);
  }
  for (size_t i = 0; i < b->message_count; i++) {
    if (b->messages[i].chat_id == chat_id) items[total++] = &b->messages[i];
  }
  if (total > 1) qsort(items, total, sizeof(items[0]), message_ptr_compare_asc);
  if (from_message_id > 0) {
    while (end < total && items[end]->message_id <= from_message_id) end++;
  } else {
    end = total;
  }
  if (end > MAG_TG_HISTORY_LIMIT) start = end - MAG_TG_HISTORY_LIMIT;
  count = end > start ? end - start : 0;
  if (count > 0) {
    oldest_id = items[start]->message_id;
    newest_id = items[end - 1]->message_id;
  }
  snprintf(out, cap,
           "Mag Telegram messages chat=%lld\n"
           "page=%s cached-total=%zu page-count=%zu oldest-id=%lld newest-id=%lld\n",
           chat_id, page, total, count, oldest_id, newest_id);
  for (size_t i = start; i < end; i++) {
    struct Message *m = items[i];
    char label[256];
    appendf(out, cap, "%lld | %s: %s\n", m->message_id, sender_label(b, m, label, sizeof(label)), m->text);
  }
  if (count == 0) appendf(out, cap, "No cached text messages for this chat/page yet.\n");
}

static void command_send(struct Bridge *b, long long chat_id, const char *text, char *out, size_t cap) {
  char req[MAG_TG_MAX_TEXT + 1024];
  if (b->live) {
    build_send_message_request(chat_id, text, req, sizeof(req));
    td_send_json(b, req);
    snprintf(out, cap, "sent text request chat=%lld\n", chat_id);
  } else {
    add_message(b, chat_id, (long long)time(NULL), 0, true, text);
    snprintf(out, cap, "mock sent chat=%lld\n", chat_id);
  }
}

static bool split_register_names(const char *value, char *first, size_t first_cap, char *last, size_t last_cap) {
  const char *p = skip_space(value ? value : "");
  size_t n = 0;
  while (p[n] && !isspace((unsigned char)p[n]) && n + 1 < first_cap) {
    first[n] = p[n];
    n++;
  }
  first[n] = 0;
  while (p[n] && !isspace((unsigned char)p[n])) n++;
  copy_str(last, last_cap, skip_space(p + n));
  return first[0] != 0;
}

static bool build_auth_request(const char *type, const char *value, char *req, size_t req_cap, char *err, size_t err_cap) {
  char escaped[MAG_TG_MAX_TEXT];
  char first[128];
  char last[128];
  char first_escaped[256];
  char last_escaped[256];
  if (!type) {
    copy_str(err, err_cap, "missing auth type");
    return false;
  }
  if (strcmp(type, "phone") == 0) {
    json_escape(value, escaped, sizeof(escaped));
    snprintf(req, req_cap, "{\"@type\":\"setAuthenticationPhoneNumber\",\"phone_number\":\"%s\"}", escaped);
  } else if (strcmp(type, "code") == 0) {
    json_escape(value, escaped, sizeof(escaped));
    snprintf(req, req_cap, "{\"@type\":\"checkAuthenticationCode\",\"code\":\"%s\"}", escaped);
  } else if (strcmp(type, "password") == 0) {
    json_escape(value, escaped, sizeof(escaped));
    snprintf(req, req_cap, "{\"@type\":\"checkAuthenticationPassword\",\"password\":\"%s\"}", escaped);
  } else if (strcmp(type, "register") == 0) {
    if (!split_register_names(value, first, sizeof(first), last, sizeof(last))) {
      copy_str(err, err_cap, "auth-register needs at least a first name");
      return false;
    }
    json_escape(first, first_escaped, sizeof(first_escaped));
    json_escape(last, last_escaped, sizeof(last_escaped));
    snprintf(req, req_cap,
             "{\"@type\":\"registerUser\",\"first_name\":\"%s\",\"last_name\":\"%s\","
             "\"disable_notification\":false}",
             first_escaped, last_escaped);
  } else {
    snprintf(err, err_cap, "unknown auth type: %s", type);
    return false;
  }
  return true;
}

static void command_auth(struct Bridge *b, const char *type, const char *value, char *out, size_t cap) {
  char req[MAG_TG_MAX_TEXT + 256];
  char err[256] = "";
  if (!build_auth_request(type, value, req, sizeof(req), err, sizeof(err))) {
    snprintf(out, cap, "%s\n", err[0] ? err : "could not build auth request");
    return;
  }
  if (!b->live) {
    response_auth_status(b, out, cap);
    return;
  }
  td_send_json(b, req);
  poll_tdlib_for(b, 800);
  response_auth_status(b, out, cap);
}

static void handle_request(struct Bridge *b, const char *request, char *out, size_t cap) {
  char cmd[64];
  const char *arg;
  long long chat_id = 0;
  out[0] = 0;
  if (!request || !request[0]) {
    snprintf(out, cap, "empty request\n");
    return;
  }
  while (*request && isspace((unsigned char)*request)) request++;
  size_t i = 0;
  while (request[i] && !isspace((unsigned char)request[i]) && i + 1 < sizeof(cmd)) {
    cmd[i] = request[i];
    i++;
  }
  cmd[i] = 0;
  arg = skip_space(request + i);
  if (strcmp(cmd, "status") == 0) response_status(b, out, cap);
  else if (strcmp(cmd, "doctor") == 0) response_doctor(out, cap);
  else if (strcmp(cmd, "auth-status") == 0) response_auth_status(b, out, cap);
  else if (strcmp(cmd, "chats") == 0) response_chats(b, out, cap);
  else if (strcmp(cmd, "messages") == 0) {
    char *end = NULL;
    long long from_message_id = 0;
    chat_id = strtoll(arg, &end, 10);
    if (end) from_message_id = strtoll(skip_space(end), NULL, 10);
    response_messages(b, chat_id, from_message_id, out, cap);
  } else if (strcmp(cmd, "older") == 0) {
    char *end = NULL;
    long long from_message_id = 0;
    chat_id = strtoll(arg, &end, 10);
    if (end) from_message_id = strtoll(skip_space(end), NULL, 10);
    response_messages(b, chat_id, from_message_id, out, cap);
  } else if (strcmp(cmd, "send") == 0) {
    char *end = NULL;
    chat_id = strtoll(arg, &end, 10);
    command_send(b, chat_id, skip_space(end ? end : ""), out, cap);
  } else if (strcmp(cmd, "auth-phone") == 0) command_auth(b, "phone", arg, out, cap);
  else if (strcmp(cmd, "auth-code") == 0) command_auth(b, "code", arg, out, cap);
  else if (strcmp(cmd, "auth-password") == 0) command_auth(b, "password", arg, out, cap);
  else if (strcmp(cmd, "auth-register") == 0) command_auth(b, "register", arg, out, cap);
  else if (strcmp(cmd, "raw") == 0) {
    if (b->live) {
      td_send_json(b, arg);
      snprintf(out, cap, "sent raw TDLib request\n");
    } else snprintf(out, cap, "raw unavailable in mock mode\n");
  } else if (strcmp(cmd, "quit") == 0) {
    b->stop = true;
    snprintf(out, cap, "stopping mag-telegram-bridge\n");
  } else {
    snprintf(out, cap, "unknown request: %s\n", cmd);
  }
}

static void maybe_handle_request_file(struct Bridge *b) {
  char *request;
  char response[8192];
  struct stat st;
  if (stat(MAG_TG_REQUEST_PATH, &st) != 0 || st.st_size <= 0) return;
  request = read_file(MAG_TG_REQUEST_PATH);
  unlink(MAG_TG_REQUEST_PATH);
  if (!request) return;
  trim_right(request);
  handle_request(b, request, response, sizeof(response));
  write_file_atomic(MAG_TG_RESPONSE_PATH, response);
  free(request);
}

static int run_daemon(bool force_mock) {
  struct Bridge b;
  memset(&b, 0, sizeof(b));
  if (daemon_already_running()) {
    fprintf(stderr, "mag-telegram-bridge daemon already running\n");
    return 0;
  }
  write_pid_file();
  copy_str(b.backend, sizeof(b.backend), "mock");
  copy_str(b.auth_state, sizeof(b.auth_state), "starting");
  signal(SIGINT, on_signal);
  signal(SIGTERM, on_signal);
  unlink(MAG_TG_REQUEST_PATH);
  unlink(MAG_TG_RESPONSE_PATH);
  if (!force_mock && init_live(&b)) {
    /* live mode initialized */
  } else {
    init_mock(&b);
  }
  fprintf(stderr, "mag-telegram-bridge daemon backend=%s auth=%s error=%s\n",
          b.backend, b.auth_state, b.last_error[0] ? b.last_error : "none");
  while (!g_stop && !b.stop) {
    poll_tdlib(&b);
    maybe_handle_request_file(&b);
    sleep_ms(100);
  }
  if (b.live) td_send_json(&b, "{\"@type\":\"close\"}");
  if (b.td.destroy && b.td.client) b.td.destroy(b.td.client);
  if (b.td.handle) dlclose(b.td.handle);
  unlink(MAG_TG_PID_PATH);
  return 0;
}

static int request_daemon_text(const char *text) {
  char request[MAG_TG_MAX_TEXT];
  char *response;
  copy_str(request, sizeof(request), text && text[0] ? text : "status");
  unlink(MAG_TG_RESPONSE_PATH);
  if (!write_file_atomic(MAG_TG_REQUEST_PATH, request)) {
    fprintf(stderr, "failed to write %s: %s\n", MAG_TG_REQUEST_PATH, strerror(errno));
    return 2;
  }
  for (int i = 0; i < 50; i++) {
    struct stat st;
    if (stat(MAG_TG_RESPONSE_PATH, &st) == 0 && st.st_size > 0) {
      response = read_file(MAG_TG_RESPONSE_PATH);
      if (response) {
        fputs(response, stdout);
        free(response);
        return 0;
      }
    }
    sleep_ms(100);
  }
  fprintf(stderr, "mag-telegram-bridge daemon did not respond; start it with: mag-telegram-bridge --daemon\n");
  return 1;
}

static int request_daemon(int argc, char **argv) {
  char request[MAG_TG_MAX_TEXT];
  request[0] = 0;
  for (int i = 1; i < argc; i++) {
    if (i > 1) appendf(request, sizeof(request), " ");
    appendf(request, sizeof(request), "%s", argv[i]);
  }
  return request_daemon_text(request);
}

static int request_daemon_file(const char *path) {
  char *request = read_file(path);
  int status;
  if (!request) {
    fprintf(stderr, "failed to read request file %s: %s\n", path, strerror(errno));
    return 2;
  }
  trim_right(request);
  status = request_daemon_text(request);
  free(request);
  return status;
}

static int doctor_command(void) {
  char out[8192];
  response_doctor(out, sizeof(out));
  fputs(out, stdout);
  return strstr(out, "status=ready-for-auth") ? 0 : 1;
}

static int self_test(void) {
  struct Bridge b;
  struct Bridge cfg;
  char out[8192];
  char req[MAG_TG_MAX_TEXT + 1024];
  char *high;
  char *low;
  char cfgpath[256];
  const char *old_config = getenv("MAG_TELEGRAM_CONFIG");
  char old_config_copy[512];
  bool had_old_config = old_config && old_config[0];
  memset(&b, 0, sizeof(b));
  memset(&cfg, 0, sizeof(cfg));
  if (had_old_config) copy_str(old_config_copy, sizeof(old_config_copy), old_config);
  snprintf(cfgpath, sizeof(cfgpath), "/tmp/mag-telegram-bridge-test-%ld.conf", (long)getpid());
  if (!write_file_atomic(cfgpath,
                         "api_id = 12345\n"
                         "api_hash = test-hash\n"
                         "encryption_key = 'test key'\n"
                         "data_dir = /tmp/mag-telegram-test-db\n"
                         "tdlib_library = /tmp/libtdjson-test.so\n")) {
    return 1;
  }
  setenv("MAG_TELEGRAM_CONFIG", cfgpath, 1);
  read_config_file(&cfg);
  if (strcmp(cfg.api_id, "12345") != 0) return 1;
  if (strcmp(cfg.api_hash, "test-hash") != 0) return 1;
  if (strcmp(cfg.encryption_key, "test key") != 0) return 1;
  if (strcmp(cfg.data_dir, "/tmp/mag-telegram-test-db") != 0) return 1;
  if (strcmp(cfg.tdlib_library, "/tmp/libtdjson-test.so") != 0) return 1;
  response_doctor(out, sizeof(out));
  if (!strstr(out, "api-id=set")) return 1;
  if (!strstr(out, "api-hash=set")) return 1;
  if (!strstr(out, "tdlib-library=/tmp/libtdjson-test.so")) return 1;
  if (!strstr(out, "tdlib-load=fail")) return 1;
  if (!strstr(out, "status=not-ready")) return 1;
  if (strstr(out, "test-hash")) return 1;
  if (had_old_config) setenv("MAG_TELEGRAM_CONFIG", old_config_copy, 1);
  else unsetenv("MAG_TELEGRAM_CONFIG");
  unlink(cfgpath);
  build_send_message_request(1234, "hello \"telegram\"", req, sizeof(req));
  if (!strstr(req, "\"@type\":\"sendMessage\"")) return 1;
  if (!strstr(req, "\"entities\":[]")) return 1;
  if (!strstr(req, "hello \\\"telegram\\\"")) return 1;
  if (!build_auth_request("register", "Alice Example", req, sizeof(req), out, sizeof(out))) return 1;
  if (!strstr(req, "\"@type\":\"registerUser\"")) return 1;
  if (!strstr(req, "\"first_name\":\"Alice\"")) return 1;
  if (!strstr(req, "\"last_name\":\"Example\"")) return 1;
  if (!strstr(req, "\"disable_notification\":false")) return 1;
  if (build_auth_request("register", "", req, sizeof(req), out, sizeof(out))) return 1;
  init_mock(&b);
  handle_request(&b, "status", out, sizeof(out));
  if (!strstr(out, "backend=mock")) return 1;
  if (!strstr(out, "auth-action=none")) return 1;
  handle_auth_update(&b, "{\"@type\":\"updateAuthorizationState\",\"authorization_state\":{\"@type\":\"authorizationStateWaitRegistration\"}}");
  handle_request(&b, "auth-status", out, sizeof(out));
  if (!strstr(out, "state=wait-registration")) return 1;
  if (!strstr(out, "action=auth-register")) return 1;
  handle_request(&b, "chats", out, sizeof(out));
  if (!strstr(out, "Saved Messages")) return 1;
  handle_td_update(&b,
                   "{\"@type\":\"updateNewChat\",\"chat\":{\"@type\":\"chat\",\"id\":4243,"
                   "\"title\":\"Sorted Low\",\"type\":{\"@type\":\"chatTypePrivate\"},"
                   "\"positions\":[{\"@type\":\"chatPosition\",\"list\":{\"@type\":\"chatListMain\"},\"order\":\"10\"}]}}");
  handle_td_update(&b,
                   "{\"@type\":\"updateNewChat\",\"chat\":{\"@type\":\"chat\",\"id\":4244,"
                   "\"title\":\"Sorted High\",\"type\":{\"@type\":\"chatTypePrivate\"},"
                   "\"positions\":[{\"@type\":\"chatPosition\",\"list\":{\"@type\":\"chatListMain\"},\"order\":\"20\"}]}}");
  handle_request(&b, "chats", out, sizeof(out));
  high = strstr(out, "Sorted High");
  low = strstr(out, "Sorted Low");
  if (!high || !low || high > low) return 1;
  handle_td_update(&b,
                   "{\"@type\":\"updateNewChat\",\"chat\":{\"@type\":\"chat\",\"id\":4250,"
                   "\"title\":\"Channel Kind\",\"type\":{\"@type\":\"chatTypeSupergroup\","
                   "\"supergroup_id\":111,\"is_channel\":true},"
                   "\"positions\":[{\"@type\":\"chatPosition\",\"list\":{\"@type\":\"chatListMain\"},\"order\":\"40\"}]}}");
  handle_td_update(&b,
                   "{\"@type\":\"updateNewChat\",\"chat\":{\"@type\":\"chat\",\"id\":4251,"
                   "\"title\":\"Supergroup Kind\",\"type\":{\"@type\":\"chatTypeSupergroup\","
                   "\"supergroup_id\":112,\"is_channel\":false},"
                   "\"positions\":[{\"@type\":\"chatPosition\",\"list\":{\"@type\":\"chatListMain\"},\"order\":\"35\"}]}}");
  handle_request(&b, "chats", out, sizeof(out));
  if (!strstr(out, "4250 [channel] Channel Kind")) return 1;
  if (!strstr(out, "4251 [supergroup] Supergroup Kind")) return 1;
  handle_td_update(&b,
                   "{\"@type\":\"updateChatPosition\",\"chat_id\":4243,"
                   "\"position\":{\"@type\":\"chatPosition\",\"list\":{\"@type\":\"chatListMain\"},\"order\":\"30\"}}");
  handle_request(&b, "chats", out, sizeof(out));
  high = strstr(out, "Sorted High");
  low = strstr(out, "Sorted Low");
  if (!high || !low || low > high) return 1;
  handle_request(&b, "send 1001 hello", out, sizeof(out));
  if (!strstr(out, "mock sent")) return 1;
  handle_request(&b, "messages 1001", out, sizeof(out));
  if (!strstr(out, "hello")) return 1;
  handle_td_update(&b,
                   "{\"@type\":\"updateUser\",\"user\":{\"@type\":\"user\",\"id\":777,"
                   "\"first_name\":\"Alice\",\"last_name\":\"Example\",\"username\":\"alice\"}}");
  handle_td_update(&b,
                   "{\"@type\":\"updateUser\",\"user\":{\"@type\":\"user\",\"id\":778,"
                   "\"first_name\":\"Bob\",\"last_name\":\"Older\",\"username\":\"bob\"}}");
  handle_td_update(&b,
                   "{\"@type\":\"messages\",\"total_count\":2,\"messages\":[{"
                   "\"@type\":\"message\",\"id\":9001,\"chat_id\":4242,"
                   "\"sender_id\":{\"@type\":\"messageSenderUser\",\"user_id\":777},"
                   "\"is_outgoing\":false,"
                   "\"content\":{\"@type\":\"messageText\","
                   "\"text\":{\"@type\":\"formattedText\",\"text\":\"history hello\",\"entities\":[]}}"
                   "},{"
                   "\"@type\":\"message\",\"id\":9000,\"chat_id\":4242,"
                   "\"sender_id\":{\"@type\":\"messageSenderUser\",\"user_id\":778},"
                   "\"is_outgoing\":false,"
                   "\"content\":{\"@type\":\"messageText\","
                   "\"text\":{\"@type\":\"formattedText\",\"text\":\"older hello\",\"entities\":[]}}"
                   "}]}");
  handle_request(&b, "messages 4242", out, sizeof(out));
  if (!strstr(out, "Alice Example: history hello")) return 1;
  if (!strstr(out, "oldest-id=9000")) return 1;
  handle_request(&b, "older 4242 9001", out, sizeof(out));
  if (!strstr(out, "Bob Older: older hello")) return 1;
  handle_td_update(&b,
                   "{\"@type\":\"updateNewMessage\",\"message\":{\"@type\":\"message\","
                   "\"id\":9010,\"chat_id\":4242,"
                   "\"sender_id\":{\"@type\":\"messageSenderUser\",\"user_id\":777},"
                   "\"is_outgoing\":false,"
                   "\"content\":{\"@type\":\"messagePhoto\","
                   "\"caption\":{\"@type\":\"formattedText\",\"text\":\"caption hello\",\"entities\":[]}}}}");
  handle_request(&b, "messages 4242", out, sizeof(out));
  if (!strstr(out, "Alice Example: [caption] caption hello")) return 1;
  puts("mag-telegram-bridge self-test ok");
  return 0;
}

int main(int argc, char **argv) {
  if (argc >= 2 && strcmp(argv[1], "--daemon") == 0) return run_daemon(false);
  if (argc >= 2 && strcmp(argv[1], "--mock-daemon") == 0) return run_daemon(true);
  if (argc >= 2 && strcmp(argv[1], "--self-test") == 0) return self_test();
  if (argc >= 2 && strcmp(argv[1], "--doctor") == 0) return doctor_command();
  if (argc >= 3 && strcmp(argv[1], "--request-file") == 0) return request_daemon_file(argv[2]);
  return request_daemon(argc, argv);
}

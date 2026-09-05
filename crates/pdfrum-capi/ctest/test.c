/* The C side of libpdfrum's proof.
 *
 * This is not a unit test of the Rust — `cargo nextest` covers that. It is the
 * thing no Rust test can do: compile a C program against the generated header,
 * link the built shared library, and exercise the contracts the header states.
 * A header that declares a symbol the library does not export, a struct whose
 * layout C reads differently than Rust wrote it, or a thread rule that does not
 * hold, is invisible to every other gate in this repository and fails here.
 *
 * The last check is the point of the whole library: eight pthreads, each taking
 * its own `pdfrum_page` of one shared `pdfrum_document`, rendering
 * concurrently, and every pixmap coming out byte-identical to a single-threaded
 * render of the same page.
 *
 * Run it through ctest/run.sh, which compiles and sets LD_LIBRARY_PATH. */

#include <pdfrum.h>

#include <pthread.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

/* How many workers the thread check runs. */
#define THREADS 8

/* The fixture this test reads, relative to the repository root. */
#define HELLO "crates/pdfrum-cli/tests/fixtures/hello_world_2_pages.pdf"
#define FORM "crates/pdfrum-cli/tests/fixtures/text_form.pdf"
#define ENCRYPTED "crates/pdfrum-cli/tests/fixtures/encrypted.pdf"

static int failures = 0;

/* One assertion, reported rather than aborted, so a run says everything that
 * is wrong instead of only the first thing. */
static void check(int ok, const char *what) {
    if (ok) {
        printf("  ok   %s\n", what);
    } else {
        printf("  FAIL %s\n", what);
        failures++;
    }
}

/* Prints and clears an error a call filled, so one `pdfrum_error` can be
 * reused down a function without leaking its message. */
static void clear(pdfrum_error *error) {
    if (error->message) {
        printf("       (%u) %s\n", (unsigned)error->code, error->message);
    }
    pdfrum_error_free(error);
}

/* The whole of a file, as bytes the caller frees. Not a library function —
 * `pdfrum_open_file` exists for this — but the byte path needs exercising and
 * a caller that already has bytes in memory is the case it is for. */
static unsigned char *slurp(const char *path, size_t *len) {
    FILE *f = fopen(path, "rb");
    if (!f) {
        return NULL;
    }
    if (fseek(f, 0, SEEK_END) != 0) {
        fclose(f);
        return NULL;
    }
    long size = ftell(f);
    if (size < 0) {
        fclose(f);
        return NULL;
    }
    rewind(f);
    unsigned char *bytes = malloc((size_t)size);
    if (!bytes) {
        fclose(f);
        return NULL;
    }
    size_t read = fread(bytes, 1, (size_t)size, f);
    fclose(f);
    if (read != (size_t)size) {
        free(bytes);
        return NULL;
    }
    *len = read;
    return bytes;
}

/* Whether a pixmap has any pixel that is not opaque white — that is, whether
 * anything was actually drawn. */
static int has_ink(const unsigned char *rgba, size_t len) {
    for (size_t i = 0; i + 3 < len; i += 4) {
        if (rgba[i] != 255 || rgba[i + 1] != 255 || rgba[i + 2] != 255) {
            return 1;
        }
    }
    return 0;
}

/* What one worker renders, and where it puts it. */
struct job {
    const pdfrum_document *document;
    unsigned char *rgba;
    size_t len;
    uint32_t width;
    uint32_t height;
    int ok;
};

/* One worker: its own page of the shared document, rendered into its own
 * buffer. Nothing here is shared but the `const pdfrum_document *`, which the
 * header says may be used from any thread and from several at once. */
static void *render_worker(void *raw) {
    struct job *job = raw;
    pdfrum_error error = {0};

    pdfrum_page *page = pdfrum_document_page(job->document, 0, &error);
    if (!page) {
        clear(&error);
        return NULL;
    }

    size_t stride = 0;
    if (!pdfrum_page_render_size(page, 2.0, &job->width, &job->height, &stride,
                                 &error)) {
        clear(&error);
        pdfrum_page_close(page);
        return NULL;
    }

    job->len = stride * (size_t)job->height;
    job->rgba = malloc(job->len);
    if (!job->rgba) {
        pdfrum_page_close(page);
        return NULL;
    }

    job->ok = pdfrum_page_render(page, 2.0, job->rgba, stride, NULL, NULL,
                                 &error);
    if (!job->ok) {
        clear(&error);
    }
    pdfrum_page_close(page);
    return NULL;
}

/* Eight threads, one shared document, a page each. The claim under test is the
 * one the library exists for, and it has two halves: every render succeeds, and
 * every render agrees byte for byte with a single-threaded one. A racing
 * renderer that merely does not crash would pass the first half and fail the
 * second. */
static void threads_render_the_same_bytes(const pdfrum_document *document) {
    printf("eight threads, one document, a page each\n");

    /* The answer to compare against, taken on this thread with nothing else
     * running. */
    struct job reference = {.document = document};
    render_worker(&reference);
    check(reference.ok, "the single-threaded reference render succeeds");
    if (!reference.ok) {
        free(reference.rgba);
        return;
    }
    check(has_ink(reference.rgba, reference.len),
          "the reference render has ink on it");

    pthread_t workers[THREADS];
    struct job jobs[THREADS];
    int spawned = 0;
    for (int i = 0; i < THREADS; i++) {
        jobs[i] = (struct job){.document = document};
        if (pthread_create(&workers[i], NULL, render_worker, &jobs[i]) == 0) {
            spawned++;
        } else {
            break;
        }
    }
    check(spawned == THREADS, "all eight workers start");
    for (int i = 0; i < spawned; i++) {
        pthread_join(workers[i], NULL);
    }

    int all_ok = 1;
    int all_same = 1;
    for (int i = 0; i < spawned; i++) {
        if (!jobs[i].ok) {
            all_ok = 0;
            continue;
        }
        if (jobs[i].len != reference.len || jobs[i].width != reference.width ||
            jobs[i].height != reference.height ||
            memcmp(jobs[i].rgba, reference.rgba, reference.len) != 0) {
            all_same = 0;
        }
    }
    check(all_ok, "every concurrent render succeeds");
    check(all_same, "every concurrent pixmap equals the single-threaded one");
    printf("       %d threads x %ux%u RGBA, all identical\n", spawned,
           reference.width, reference.height);

    for (int i = 0; i < spawned; i++) {
        free(jobs[i].rgba);
    }
    free(reference.rgba);
}

/* Opening bytes, counting pages, rendering, and reading the text back. */
static void the_basics(void) {
    printf("open, count, render, extract\n");
    pdfrum_error error = {0};

    size_t len = 0;
    unsigned char *bytes = slurp(HELLO, &len);
    check(bytes != NULL, "the fixture reads");
    if (!bytes) {
        return;
    }

    pdfrum_document *document = pdfrum_open(bytes, len, NULL, &error);
    check(document != NULL, "pdfrum_open accepts the bytes");
    /* The header says the bytes are copied, so the caller's buffer is free the
     * moment the open returns. Freeing it here rather than at the end is the
     * test of that promise: everything below reads the document. */
    free(bytes);
    if (!document) {
        clear(&error);
        return;
    }

    check(pdfrum_page_count(document) == 2, "the document has two pages");

    pdfrum_page *page = pdfrum_document_page(document, 0, &error);
    check(page != NULL, "page one opens");
    if (page) {
        double width = 0;
        double height = 0;
        pdfrum_page_size(page, &width, &height);
        check(width > 0 && height > 0, "the page has a size");

        uint32_t pixel_width = 0;
        uint32_t pixel_height = 0;
        size_t stride = 0;
        if (pdfrum_page_render_size(page, 1.0, &pixel_width, &pixel_height,
                                    &stride, &error)) {
            unsigned char *rgba = malloc(stride * (size_t)pixel_height);
            check(rgba != NULL, "the caller's buffer allocates");
            if (rgba) {
                check(pdfrum_page_render(page, 1.0, rgba, stride, NULL, NULL,
                                         &error),
                      "the page renders into it");
                check(has_ink(rgba, stride * (size_t)pixel_height),
                      "some pixel is not white");
                free(rgba);
            }
        } else {
            clear(&error);
        }

        char *text = pdfrum_page_text(page, &error);
        check(text != NULL, "the page's text comes back");
        if (text) {
            check(strstr(text, "Hello, world!") != NULL,
                  "the text contains \"Hello, world!\"");
            pdfrum_free(text);
        } else {
            clear(&error);
        }

        pdfrum_words *words = pdfrum_page_words(page, &error);
        check(words != NULL, "the page's words come back");
        if (words) {
            size_t count = pdfrum_words_count(words);
            check(count >= 2, "there are at least two words");
            pdfrum_word word;
            check(pdfrum_words_get(words, 0, &word),
                  "the first word reads out");
            check(word.text != NULL && word.text[0] != '\0',
                  "the first word has text");
            check(!pdfrum_words_get(words, count, &word),
                  "an index past the end is refused, not a crash");
            printf("       %zu words, the first is \"%s\"\n", count, word.text);
            /* One free for the lot, and the strings die with it. */
            pdfrum_words_free(words);
        } else {
            clear(&error);
        }

        pdfrum_hits *hits = pdfrum_page_search(page, "world", true, &error);
        check(hits != NULL, "the search runs");
        if (hits) {
            check(pdfrum_hits_count(hits) >= 1, "it finds \"world\"");
            pdfrum_hits_free(hits);
        } else {
            clear(&error);
        }

        pdfrum_page_close(page);
    } else {
        clear(&error);
    }

    /* A page past the end is an error with a code, not a crash. */
    pdfrum_page *missing = pdfrum_document_page(document, 99, &error);
    check(missing == NULL, "a page past the end is refused");
    check(error.code != PDFRUM_CODE_OK, "and it says why");
    clear(&error);

    pdfrum_close(document);
}

/* Filling a form, saving it to bytes, and reading the value back out of those
 * bytes — the round trip, not just the write. */
static void fill_a_form_and_read_it_back(void) {
    printf("fill a form, save it, reopen the bytes\n");
    pdfrum_error error = {0};

    pdfrum_document *document = pdfrum_open_file(FORM, NULL, &error);
    check(document != NULL, "the form fixture opens");
    if (!document) {
        clear(&error);
        return;
    }

    pdfrum_form *form = pdfrum_form_open(document);
    check(form != NULL, "it has a form");
    if (!form) {
        pdfrum_close(document);
        return;
    }

    size_t fields = pdfrum_form_field_count(form);
    check(fields >= 1, "the form has a field");

    pdfrum_field field;
    check(pdfrum_form_field(form, 0, &field, &error),
          "the first field reads out");
    if (!field.name) {
        clear(&error);
        pdfrum_form_free(form);
        pdfrum_close(document);
        return;
    }
    /* The header says `name` is borrowed and the next call replaces it, so a
     * value that must outlive the call is copied. */
    char name[256];
    snprintf(name, sizeof name, "%s", field.name);
    printf("       field 0 is \"%s\", kind %u\n", name, (unsigned)field.kind);

    check(pdfrum_form_set(form, name, "written by the C test", &error),
          "a value writes into it");

    check(!pdfrum_form_set(form, "no such field", "x", &error),
          "an unknown field is refused");
    check(error.code == PDFRUM_CODE_ARGUMENT, "with the argument code");
    clear(&error);

    /* The write is buffered, so reading the field back through the same handle
     * must already show it — before anything is saved. */
    check(pdfrum_form_field(form, 0, &field, &error),
          "the field reads out again");
    check(field.value && strcmp(field.value, "written by the C test") == 0,
          "and already shows the buffered value");

    pdfrum_buffer saved = {0};
    pdfrum_save_options options = {.deterministic = true};
    check(pdfrum_form_save(form, &options, &saved, &error),
          "the filled form saves to bytes");
    if (!saved.data) {
        clear(&error);
        pdfrum_form_free(form);
        pdfrum_close(document);
        return;
    }
    check(saved.len > 0, "the saved bytes are not empty");

    /* The round trip: reopen what was written and ask a fresh form for the
     * value. Nothing of the first document is consulted. */
    pdfrum_document *reopened = pdfrum_open(saved.data, saved.len, NULL, &error);
    check(reopened != NULL, "the saved bytes reopen as a document");
    if (reopened) {
        pdfrum_form *again = pdfrum_form_open(reopened);
        check(again != NULL, "the reopened document has a form");
        if (again) {
            pdfrum_field round_trip;
            check(pdfrum_form_field(again, 0, &round_trip, &error),
                  "its first field reads out");
            check(round_trip.value &&
                      strcmp(round_trip.value, "written by the C test") == 0,
                  "and carries the value the C test wrote");
            pdfrum_form_free(again);
        }
        pdfrum_close(reopened);
    } else {
        clear(&error);
    }

    pdfrum_free(saved.data);
    pdfrum_form_free(form);
    pdfrum_close(document);
}

/* A wrong password is code 3 with a message — the error contract, exercised
 * rather than described. */
static void a_wrong_password_reports_itself(void) {
    printf("a wrong password\n");
    pdfrum_error error = {0};

    pdfrum_document *document =
        pdfrum_open_file(ENCRYPTED, "definitely not the password", &error);
    check(document == NULL, "the open fails");
    check(error.code == PDFRUM_CODE_WRONG_PASSWORD,
          "the code is PDFRUM_CODE_WRONG_PASSWORD (3)");
    check(error.message != NULL, "and there is a message");
    if (error.message) {
        printf("       (%u) %s\n", (unsigned)error.code, error.message);
    }

    pdfrum_error_free(&error);
    check(error.message == NULL, "freeing the error clears the message");
    /* Freeing twice is defined and does nothing. */
    pdfrum_error_free(&error);

    if (document) {
        pdfrum_close(document);
    }
}

/* Nulls answer rather than crash, everywhere the header says they may. */
static void nulls_are_answered(void) {
    printf("nulls are answered, not crashed on\n");
    pdfrum_error error = {0};

    check(pdfrum_page_count(NULL) == 0, "a null document counts zero pages");
    check(pdfrum_document_page(NULL, 0, &error) == NULL, "a null document has no page");
    check(error.code == PDFRUM_CODE_ARGUMENT, "and says the argument was bad");
    clear(&error);

    check(pdfrum_page_text(NULL, &error) == NULL, "a null page has no text");
    clear(&error);

    check(pdfrum_words_count(NULL) == 0, "a null word list is empty");

    /* Every free tolerates null, so a caller need not guard a pointer a failed
     * call left behind. */
    pdfrum_free(NULL);
    pdfrum_page_close(NULL);
    pdfrum_close(NULL);
    pdfrum_form_free(NULL);
    pdfrum_words_free(NULL);
    pdfrum_cancel_free(NULL);
    pdfrum_error_free(NULL);
    check(1, "every free tolerates a null pointer");

    /* A null out-parameter is allowed where the header says so. */
    pdfrum_page_size(NULL, NULL, NULL);
    check(1, "pdfrum_page_size tolerates a null page and null outs");
}

int main(void) {
    printf("libpdfrum %s — the C test\n\n", pdfrum_version());

    the_basics();
    printf("\n");
    fill_a_form_and_read_it_back();
    printf("\n");
    a_wrong_password_reports_itself();
    printf("\n");
    nulls_are_answered();
    printf("\n");

    /* The thread check gets its own document, opened once and shared. */
    pdfrum_error error = {0};
    pdfrum_document *shared = pdfrum_open_file(HELLO, NULL, &error);
    if (shared) {
        threads_render_the_same_bytes(shared);
        pdfrum_close(shared);
    } else {
        check(0, "the shared document opens for the thread check");
        clear(&error);
    }

    printf("\n");
    if (failures == 0) {
        printf("the C test passed.\n");
        return 0;
    }
    printf("the C test FAILED: %d checks did not hold.\n", failures);
    return 1;
}

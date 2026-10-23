/* Peak RSS of one command, without the launcher's own memory in it (a child forked from a big
 * Python process inherits its RSS high-water mark; one forked from this tiny program does not).
 *   maxrss OUT_FILE cmd [args...]   -> runs cmd, writes "<maxrss_kb> <exit status>" to OUT_FILE,
 *                                      exits with cmd's exit code (128+signal when killed). */
#include <stdio.h>
#include <stdlib.h>
#include <sys/resource.h>
#include <sys/wait.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc < 3) { fprintf(stderr, "usage: maxrss OUT cmd...\n"); return 2; }
    pid_t pid = fork();
    if (pid < 0) { perror("fork"); return 2; }
    if (pid == 0) { execvp(argv[2], argv + 2); perror("exec"); _exit(127); }
    int st; struct rusage ru;
    if (wait4(pid, &st, 0, &ru) < 0) { perror("wait4"); return 2; }
    FILE *f = fopen(argv[1], "w");
    if (f) { fprintf(f, "%ld %d\n", ru.ru_maxrss, st); fclose(f); }
    if (WIFEXITED(st)) return WEXITSTATUS(st);
    return 128 + WTERMSIG(st);
}

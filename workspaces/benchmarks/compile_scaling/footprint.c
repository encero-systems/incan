/* Observe one owned process, including the kernel's lifetime maximum after exit. */
#include <errno.h>
#include <libproc.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <sys/wait.h>
#include <time.h>
#include <unistd.h>

int main(int argc, char **argv) {
    if (argc < 2) return 2;
    pid_t child = fork();
    if (child < 0) { perror("fork"); return 2; }
    if (child == 0) {
        execvp(argv[1], argv + 1);
        perror("execvp"); _exit(127);
    }
    uint64_t peak = 0;
    unsigned readings = 0;
    int final_read = 0;
    const struct timespec interval = {0, 100000000};
    for (;;) {
        siginfo_t state = {0};
        if (waitid(P_PID, child, &state, WEXITED | WNOHANG | WNOWAIT) < 0) {
            if (errno == EINTR) continue;
            perror("waitid"); return 2;
        }
        struct rusage_info_v4 info = {0};
        if (proc_pid_rusage(child, RUSAGE_INFO_V4, (rusage_info_t *)&info) == 0) {
            if (info.ri_lifetime_max_phys_footprint > peak) peak = info.ri_lifetime_max_phys_footprint;
            readings++;
            if (state.si_pid == child) final_read = 1;
        }
        if (state.si_pid == child) break;
        nanosleep(&interval, NULL);
    }
    int status = 0;
    if (waitpid(child, &status, 0) < 0) { perror("waitpid"); return 2; }
    fprintf(stderr, "footprint: {\"bytes\":%llu,\"readings\":%u,\"final_read\":%d,\"pid\":%d}\n",
            (unsigned long long)peak, readings, final_read, child);
    if (!readings || !final_read) return 2;
    return WIFEXITED(status) ? WEXITSTATUS(status) : 128 + WTERMSIG(status);
}

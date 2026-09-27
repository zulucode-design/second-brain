#include <fcntl.h>
#include <linux/uinput.h>
#include <stdio.h>
#include <sys/ioctl.h>
#include <unistd.h>

static int emit(int fd, int type, int code, int value) {
    struct input_event event = {0};
    event.type = type;
    event.code = code;
    event.value = value;
    return write(fd, &event, sizeof(event)) == sizeof(event) ? 0 : -1;
}

int main(void) {
    int fd = open("/dev/uinput", O_WRONLY | O_NONBLOCK);
    if (fd < 0) { perror("open uinput"); return 1; }
    if (ioctl(fd, UI_SET_EVBIT, EV_KEY) < 0 || ioctl(fd, UI_SET_KEYBIT, KEY_X) < 0 ||
        ioctl(fd, UI_SET_KEYBIT, KEY_ESC) < 0 || ioctl(fd, UI_SET_KEYBIT, KEY_A) < 0 ||
        ioctl(fd, UI_SET_EVBIT, EV_SYN) < 0) { perror("configure uinput"); close(fd); return 1; }
    struct uinput_setup setup = {0};
    setup.id.bustype = BUS_USB;
    setup.id.vendor = 0x1;
    setup.id.product = 0x1;
    snprintf(setup.name, sizeof(setup.name), "Second Brain performance keyboard");
    if (ioctl(fd, UI_DEV_SETUP, &setup) < 0 || ioctl(fd, UI_DEV_CREATE) < 0) { perror("create uinput"); close(fd); return 1; }
    int status = 0;
    usleep(500000);
    for (int i = 0; i < 200; i++) {
        if (emit(fd, EV_KEY, KEY_X, 1) || emit(fd, EV_SYN, SYN_REPORT, 0) ||
            emit(fd, EV_KEY, KEY_X, 0) || emit(fd, EV_SYN, SYN_REPORT, 0)) { perror("emit key"); status = 1; break; }
        usleep(50000);
    }
    usleep(100000);
    if (ioctl(fd, UI_DEV_DESTROY) < 0) { perror("destroy uinput"); status = 1; }
    close(fd);
    return status;
}

/* neochumby-scummvm: the C ABI between the ScummVM backend (C++, csrc/)
 * and the platform layer (Rust, src/platform/).
 *
 * Copyright (C) 2026 the neochumby-scummvm authors
 *
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version. See LICENSE.
 *
 * The Rust side owns the devices: the screen, the touchscreen and the top
 * button, the speaker, and the clock. The C++ side is ScummVM's OSystem: it
 * scales the game into the screen, turns touches into mouse events and
 * mixes the audio when the Rust audio thread asks for it.
 */

#ifndef NEOCHUMBY_SCUMMVM_CHUMBY_H
#define NEOCHUMBY_SCUMMVM_CHUMBY_H

#include <stdint.h>

#ifdef __cplusplus
extern "C" {
#endif

#define NC_W 320
#define NC_H 240

enum nc_event_type {
	/* A finger down, or moved while down, at (x, y) in screen pixels. */
	NC_TOUCH_DOWN = 1,
	/* The finger lifted, last seen at (x, y). */
	NC_TOUCH_UP = 2,
	/* The top button held a moment: ScummVM's menu (save, load, quit). */
	NC_MENU = 3,
	/* The top button held longer, or the session ending the app: quit. */
	NC_QUIT = 4,
	/* A short press of the top button: the Escape key (skips a cutscene). */
	NC_ESCAPE = 5
};

struct nc_event {
	int32_t type;
	int32_t x;
	int32_t y;
};

/* ---- Implemented in Rust ---- */

/* The screen: NC_W x NC_H RGB565, NC_W pixels a line. On the Chumby it is
 * /dev/fb0 itself, so what is written there shows at once. */
uint16_t *nc_screen(void);
/* The rectangle of nc_screen() just finished drawing (the host test mode
 * counts frames with it; the Chumby has nothing to do). */
void nc_present(int32_t x, int32_t y, int32_t w, int32_t h);
/* Milliseconds since start, from the monotonic clock. */
uint32_t nc_millis(void);
/* Sleeps about ms milliseconds (poll() with no descriptors: the Chumby's
 * kernel can fail a restarted nanosleep with EINVAL). */
void nc_sleep_ms(uint32_t ms);
/* The next input event, if any: returns 1 and fills *ev, else 0. */
int32_t nc_poll(struct nc_event *ev);
/* Starts the audio thread at rate Hz, 16-bit stereo, which calls
 * scummvm_chumby_mix for each buffer of frames frames. Returns the rate
 * in use. Without a speaker (busy, or none) the thread still runs, at the
 * same pace, and the audio is dropped: the games wait for their sounds to
 * end, so they must keep playing. */
uint32_t nc_audio_start(uint32_t rate, uint32_t frames);
/* Stops the audio thread (waiting for it) and restores the speaker:
 * scummvm_chumby_mix is not called again. */
void nc_audio_stop(void);
/* Ends the process: stops the audio (restoring the speaker's volume and
 * mute) and exits with code. */
void nc_exit(int32_t code);

/* ---- Implemented in C++ ---- */

/* Fills frames stereo frames (interleaved int16) of audio. */
void scummvm_chumby_mix(int16_t *buf, uint32_t frames);
/* ScummVM's main, on the Chumby backend. */
int scummvm_chumby_main(int argc, char **argv);

#ifdef __cplusplus
}
#endif

#endif

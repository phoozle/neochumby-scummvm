/* neochumby-scummvm: ScummVM's OSystem for the Chumby Classic.
 *
 * Copyright (C) 2026 the neochumby-scummvm authors
 *
 * This program is free software: you can redistribute it and/or modify
 * it under the terms of the GNU General Public License as published by
 * the Free Software Foundation, either version 3 of the License, or
 * (at your option) any later version. See LICENSE.
 *
 * Modelled on ScummVM's null backend (backends/platform/null). The devices
 * are the Rust platform layer's (chumby.h); this file is ScummVM's side:
 * graphics (chumby-graphics.cpp), touches as mouse events, the mixer fed
 * from the platform's audio thread, the clock, and the managers ScummVM
 * provides for POSIX (files, saves, timers, mutexes).
 */

#define FORBIDDEN_SYMBOL_ALLOW_ALL
#include "common/scummsys.h"

#include "chumby.h"
#include "chumby-graphics.h"

#include "audio/mixer_intern.h"
#include "backends/events/default/default-events.h"
#include "backends/fs/posix/posix-fs-factory.h"
#include "backends/mixer/mixer.h"
#include "backends/modular-backend.h"
#include "backends/mutex/pthread/pthread-mutex.h"
#include "backends/saves/default/default-saves.h"
#include "backends/timer/default/default-timer.h"
#include "base/main.h"
#include "common/events.h"
#include "common/queue.h"

#include <stdio.h>
#include <stdlib.h>
#include <time.h>

/* The mixer the audio thread fills buffers from, once there is one. The
 * thread only reads it; it is set once, before the mixer is made ready,
 * and outlives the thread (nc_exit stops the thread before anything is
 * destroyed). */
static Audio::MixerImpl *volatile g_mixer = nullptr;

extern "C" void scummvm_chumby_mix(int16_t *buf, uint32_t frames) {
	Audio::MixerImpl *m = g_mixer;
	if (m)
		m->mixCallback((byte *)buf, frames * 4);
	else
		memset(buf, 0, frames * 4);
}

class ChumbyMixerManager : public MixerManager {
public:
	void init() override {
		// 22050 Hz stereo: the HE games' own rate, so their sounds are mixed
		// without resampling. 1024 frames is 46 ms a buffer.
		const uint frames = 1024;
		uint rate = nc_audio_start(22050, frames);
		_mixer = new Audio::MixerImpl(rate, true, frames);
		g_mixer = _mixer;
		_mixer->setReady(true);
	}
	void suspendAudio() override { _audioSuspended = true; }
	int resumeAudio() override {
		if (!_audioSuspended)
			return -2;
		_audioSuspended = false;
		return 0;
	}
};

class OSystem_Chumby : public ModularMixerBackend, public ModularGraphicsBackend, Common::EventSource {
public:
	OSystem_Chumby() : _touching(false) { _fsFactory = new POSIXFilesystemFactory(); }

	void initBackend() override {
		_timerManager = new DefaultTimerManager();
		_eventManager = new DefaultEventManager(this);
		_savefileManager = new DefaultSaveFileManager();
		_graphicsManager = new ChumbyGraphicsManager();
		_mixerManager = new ChumbyMixerManager();
		_mixerManager->init();
		BaseBackend::initBackend();
	}

	bool pollEvent(Common::Event &event) override;

	Common::MutexInternal *createMutex() override { return createPthreadMutexInternal(); }
	uint32 getMillis(bool skipRecord = false) override { return nc_millis(); }
	void delayMillis(uint msecs) override { nc_sleep_ms(msecs); }

	void getTimeAndDate(TimeDate &td, bool skipRecord = false) const override {
		time_t now = time(nullptr);
		struct tm t;
		localtime_r(&now, &t);
		td.tm_sec = t.tm_sec;
		td.tm_min = t.tm_min;
		td.tm_hour = t.tm_hour;
		td.tm_mday = t.tm_mday;
		td.tm_mon = t.tm_mon;
		td.tm_year = t.tm_year;
		td.tm_wday = t.tm_wday;
	}

	void quit() override { nc_exit(0); }

	void logMessage(LogMessageType::Type type, const char *message) override {
		fputs(message, stderr);
		fflush(stderr);
	}

	// HOME is /tmp under the neochumby session; the platform layer also
	// passes --config, so this is only a fallback.
	Common::Path getDefaultConfigFileName() override {
		const char *home = getenv("HOME");
		return Common::Path(home ? home : "/tmp").join("scummvm.ini");
	}

	void addSysArchivesToSearchSet(Common::SearchSet &s, int priority) override {}

private:
	ChumbyGraphicsManager *gfx() { return (ChumbyGraphicsManager *)_graphicsManager; }

	struct Pending {
		Common::Event event;
		uint32 notBefore;
	};
	void queueMouse(Common::EventType type, Common::Point p, uint32 notBefore) {
		Pending e;
		e.event.type = type;
		e.event.mouse = p;
		e.notBefore = notBefore;
		_pending.push(e);
	}

	Common::Queue<Pending> _pending;
	bool _touching;
	Common::Point _last;
};

/* A press of the finger is the mouse moving there, then the left button
 * going down a moment later, so that the game has seen the cursor over
 * what was touched (some scripts look at what is under the cursor on the
 * frame before the click); moving while pressed drags; lifting releases.
 * The top button is Escape (skips a cutscene), held a moment ScummVM's
 * menu (save, load, quit), and held longer quits: the platform layer
 * (src/input.rs) tells them apart. */
bool OSystem_Chumby::pollEvent(Common::Event &event) {
	((DefaultTimerManager *)getTimerManager())->checkTimers();

	const uint32 now = nc_millis();
	nc_event e;
	while (nc_poll(&e)) {
		switch (e.type) {
		case NC_TOUCH_DOWN: {
			Common::Point p = gfx()->screenToVirtual(e.x, e.y);
			if (!_touching) {
				_touching = true;
				queueMouse(Common::EVENT_MOUSEMOVE, p, now);
				queueMouse(Common::EVENT_LBUTTONDOWN, p, now + 50);
			} else if (p != _last) {
				queueMouse(Common::EVENT_MOUSEMOVE, p, now);
			}
			_last = p;
			break;
		}
		case NC_TOUCH_UP:
			if (_touching) {
				_touching = false;
				queueMouse(Common::EVENT_LBUTTONUP, _last, now);
			}
			break;
		case NC_MENU: {
			Pending m;
			m.event.type = Common::EVENT_MAINMENU;
			m.notBefore = now;
			_pending.push(m);
			break;
		}
		case NC_ESCAPE: {
			Pending k;
			k.event.type = Common::EVENT_KEYDOWN;
			k.event.kbd = Common::KeyState(Common::KEYCODE_ESCAPE, Common::ASCII_ESCAPE);
			k.notBefore = now;
			_pending.push(k);
			k.event.type = Common::EVENT_KEYUP;
			k.notBefore = now + 50;
			_pending.push(k);
			break;
		}
		case NC_QUIT: {
			Pending q;
			q.event.type = Common::EVENT_QUIT;
			q.notBefore = now;
			_pending.push(q);
			break;
		}
		default:
			break;
		}
	}

	// In order: a click waits for its own press, so its release does too.
	if (_pending.empty() || (int32)(now - _pending.front().notBefore) < 0)
		return false;
	event = _pending.pop().event;
	switch (event.type) {
	case Common::EVENT_MOUSEMOVE:
	case Common::EVENT_LBUTTONDOWN:
	case Common::EVENT_LBUTTONUP:
		gfx()->setMousePosition(event.mouse.x, event.mouse.y);
		break;
	default:
		break;
	}
	return true;
}

extern "C" int scummvm_chumby_main(int argc, char **argv) {
	g_system = new OSystem_Chumby();
	int res = scummvm_main(argc, argv);
	// The audio thread mixes from the backend's mixer: stop it first.
	nc_audio_stop();
	g_mixer = nullptr;
	g_system->destroy();
	return res;
}

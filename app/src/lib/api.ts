// Typed wrappers around the Tauri invoke / event surface exposed
// by `src-tauri/src/bridge.rs`. This is the only module in the
// frontend that talks to the daemon; pages and components call
// through here rather than reaching for `@tauri-apps/api` directly.

import { invoke } from '@tauri-apps/api/core';
import { listen, type UnlistenFn } from '@tauri-apps/api/event';

/** An application row sourced from `net.ludex.Tracker1.ListApplications`. */
export interface ApplicationSummary {
    id: number;
    launcher_type: string;
    launcher_id: string;
    product_name: string;
    /** Empty string when the publisher is unknown. */
    publisher: string;
    total_full_seconds: number;
    total_interactive_seconds: number;
    run_count: number;
    /** RFC 3339 UTC timestamp; empty string for never-played apps. */
    last_played_at: string;
}

/** A session row sourced from `net.ludex.Tracker1.ListRecentSessions`. */
export interface SessionSummary {
    id: number;
    application_id: number;
    product_name: string;
    started_at: string;
    /** Empty string while the session is still open. */
    ended_at: string;
    full_runtime_seconds: number;
    interactive_runtime_seconds: number;
    /** Empty string while the session is still open. */
    exit_reason: string;
}

/** Payload of the `ludex:session-ended` event. */
export interface SessionEndedPayload {
    application_id: number;
    full_runtime_seconds: number;
    interactive_runtime_seconds: number;
}

export async function listApplications(): Promise<ApplicationSummary[]> {
    return invoke<ApplicationSummary[]>('list_applications');
}

/**
 * Look up one application by id. D-Bus lacks a clean "optional"
 * primitive, so the daemon returns a 0-or-1-element array and we
 * expose the same shape.
 */
export async function getApplication(
    id: number,
): Promise<ApplicationSummary[]> {
    return invoke<ApplicationSummary[]>('get_application', { id });
}

export async function listRecentSessions(limit = 20): Promise<SessionSummary[]> {
    return invoke<SessionSummary[]>('list_recent_sessions', { limit });
}

export async function listSessionsForApplication(
    applicationId: number,
    limit = 50,
): Promise<SessionSummary[]> {
    return invoke<SessionSummary[]>('list_sessions_for_application', {
        applicationId,
        limit,
    });
}

export function onApplicationAdded(
    cb: (applicationId: number) => void,
): Promise<UnlistenFn> {
    return listen<number>('ludex:application-added', (event) => cb(event.payload));
}

export function onSessionStarted(
    cb: (applicationId: number) => void,
): Promise<UnlistenFn> {
    return listen<number>('ludex:session-started', (event) => cb(event.payload));
}

export function onSessionEnded(
    cb: (payload: SessionEndedPayload) => void,
): Promise<UnlistenFn> {
    return listen<SessionEndedPayload>('ludex:session-ended', (event) =>
        cb(event.payload),
    );
}

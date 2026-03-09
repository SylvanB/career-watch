// Smoke test: ensures the router and all its dependencies compile correctly.
// The router now takes AppState, so we just verify it type-checks.
// Full integration tests require a running database and are out of scope here.
use career_watch::routes;
use career_watch::AppState;

fn _router_type_checks_fn(_state: AppState) {
    let _router = routes::router(_state);
}

// Package ffi provides common Router methods that delegate to the implementation.
package ffi

// NewRouter creates a Router wrapping the given implementation.
func NewRouter(impl routerImpl) *Router {
	return &Router{impl: impl}
}

// Route performs a routing decision.
func (r *Router) Route(req *RouteRequest) (*RouteResult, error) {
	return r.impl.Route(req)
}

// UpdateStats sends model performance statistics to the routing engine.
func (r *Router) UpdateStats(statsJSON string) error {
	return r.impl.UpdateStats(statsJSON)
}

// Health returns the router's health status.
func (r *Router) Health() *HealthStatus {
	return r.impl.Health()
}

// Version returns the router engine version string.
func (r *Router) Version() string {
	return r.impl.Version()
}

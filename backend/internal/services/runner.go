package services

import (
	"bufio"
	"context"
	"encoding/json"
	"fmt"
	"io"
	"log/slog"
	"os"
	"os/exec"
	"sync"

	"github.com/Petr1Furious/potato-launcher/backend/internal/config"
	"github.com/Petr1Furious/potato-launcher/backend/internal/models"
)

type SpecProvider interface {
	GetSpec() (*models.BuilderSpec, error)
}

type RunnerService struct {
	cfg     *config.Config
	store   SpecProvider
	status  models.BuildStatus
	mu      sync.RWMutex
	running bool
	logger  *slog.Logger
	hub     *Hub
}

func NewRunnerService(cfg *config.Config, store SpecProvider, logger *slog.Logger, hub *Hub) *RunnerService {
	return &RunnerService{
		cfg:    cfg,
		store:  store,
		status: models.BuildIdle,
		logger: logger,
		hub:    hub,
	}
}

func (r *RunnerService) Status() models.BuildStatus {
	r.mu.RLock()
	defer r.mu.RUnlock()
	return r.status
}

func (r *RunnerService) RunBuild(ctx context.Context) error {
	r.mu.Lock()
	if r.running {
		r.mu.Unlock()
		return fmt.Errorf("build already running")
	}
	r.running = true
	r.status = models.BuildRunning
	r.mu.Unlock()

	go r.execute(context.Background())
	return nil
}

func (r *RunnerService) execute(ctx context.Context) {
	r.logger.Info("starting build process")
	r.broadcastLog("Starting build process...")

	if err := r.prepareSpecFile(); err != nil {
		r.finish(err)
		return
	}

	cmd := exec.CommandContext(
		ctx,
		r.cfg.InstanceBuilderBinary,
		"-s",
		r.cfg.SpecFile,
		r.cfg.GeneratedDir,
		r.cfg.WorkdirDir,
	)

	stdout, _ := cmd.StdoutPipe()
	stderr, _ := cmd.StderrPipe()

	if err := cmd.Start(); err != nil {
		r.finish(err)
		return
	}

	var wg sync.WaitGroup
	wg.Add(2)
	go func() {
		defer wg.Done()
		r.streamLog(stdout)
	}()
	go func() {
		defer wg.Done()
		r.streamLog(stderr)
	}()

	err := cmd.Wait()
	wg.Wait()
	r.finish(err)
}

func (r *RunnerService) streamLog(pipe io.ReadCloser) {
	scanner := bufio.NewScanner(pipe)
	for scanner.Scan() {
		text := scanner.Text()
		r.logger.Debug("build log", "line", text)
		r.broadcastLog(text)
	}
}

func (r *RunnerService) broadcastLog(text string) {
	r.hub.Broadcast(map[string]interface{}{
		"type":    "build_log",
		"message": text,
	})
}

func (r *RunnerService) finish(runErr error) {
	r.mu.Lock()
	r.running = false
	r.status = models.BuildIdle
	r.mu.Unlock()

	if runErr != nil {
		r.logger.Error("runner failed", "error", runErr)
		r.broadcastLog(fmt.Sprintf("Build failed: %v", runErr))
	} else {
		r.logger.Info("build finished successfully")
		r.broadcastLog("Build finished successfully")
	}
}

func (r *RunnerService) prepareSpecFile() error {
	spec, err := r.store.GetSpec()
	if err != nil {
		return err
	}

	builderSpec := models.BuilderSpec{
		DownloadServerBase:  r.cfg.DownloadServerBase,
		ResourcesURLBase:    r.cfg.ResourcesURLBase,
		ReplaceDownloadURLs: spec.ReplaceDownloadURLs,
		Instances:           spec.Instances,
	}
	raw, err := json.MarshalIndent(builderSpec, "", "    ")
	if err != nil {
		return err
	}
	return os.WriteFile(r.cfg.SpecFile, raw, 0o644)
}

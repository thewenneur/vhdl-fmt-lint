;;; emacs.el --- Batch test of editors/emacs/vfl.el with Eglot -*- lexical-binding: t; -*-

;; Run by run.sh: emacs --batch -l emacs.el
;; Checks that Eglot starts vfl for VHDL and SystemVerilog buffers, that
;; diagnostics arrive, and that formatting goes through the server.

(require 'cl-lib)
(defconst test-dir (file-name-directory (or load-file-name buffer-file-name)))
(add-to-list 'load-path (expand-file-name "../emacs" test-dir))
(require 'eglot)
(require 'vfl)
(setq eglot-sync-connect 20
      eglot-connect-timeout 20
      make-backup-files nil)

(defun test-fail (fmt &rest args)
  (princ (concat "FAIL: " (apply #'format fmt args) "\n") #'external-debugging-output)
  (kill-emacs 1))

(defun test-wait (pred)
  "Wait up to 20 seconds for PRED to return non-nil."
  (let ((deadline (+ (float-time) 20)) result)
    (while (and (not (setq result (funcall pred))) (< (float-time) deadline))
      (accept-process-output nil 0.05))
    result))

(defun test-open (name)
  (find-file (expand-file-name (concat "fixture/" name) test-dir))
  ;; `eglot-ensure' waits for a command loop that batch mode lacks.
  (unless (eglot-current-server)
    (apply #'eglot (eglot--guess-contact)))
  (unless (test-wait (lambda () (eglot-current-server)))
    (test-fail "vfl did not start for %s (%s)" name major-mode))
  (current-buffer))

(with-current-buffer (test-open "top.vhd")
  (unless (test-wait
           (lambda ()
             (cl-some (lambda (d) (string-match-p "unused_s" (flymake-diagnostic-text d)))
                      (flymake-diagnostics))))
    (test-fail "no diagnostic for unused_s in top.vhd"))
  (princ "ok: VHDL diagnostics\n"))

(with-current-buffer (test-open "counter.sv")
  (eglot-format-buffer)
  (unless (string-match-p "q <= q \\+ 1;" (buffer-string))
    (test-fail "formatting counter.sv did not reformat it:\n%s" (buffer-string)))
  (set-buffer-modified-p nil)
  (princ "ok: SystemVerilog formatting\n"))

(eglot-shutdown-all)
(kill-emacs 0)

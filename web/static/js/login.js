// NanoKVM Login JavaScript

const API_BASE = '/api';
let capabilities = null;
let currentState = 'password'; // password, setup, passkey, qr, recovery, recovery_display

// Initialize on page load
document.addEventListener('DOMContentLoaded', async () => {
    await checkCapabilities();
    setupEventListeners();
});

// Check system capabilities
async function checkCapabilities() {
    try {
        const response = await fetch(`${API_BASE}/system/capabilities`);
        capabilities = await response.json();
        updateUI();
    } catch (error) {
        console.error('Failed to check capabilities:', error);
        showPasswordOnly();
    }
}

// Update UI based on capabilities
function updateUI() {
    // Hide all sections first
    hideAllSections();

    if (!capabilities.tailscale_installed) {
        // Tailscale not installed
        showSection('tailscale-missing-section');
    } else if (!capabilities.tailscale_connected) {
        // Tailscale installed but not connected
        showSection('tailscale-missing-section');
    } else if (!capabilities.tailscale_funnel_active) {
        // Tailscale connected but funnel not active - show setup
        showSection('setup-section');
    } else if (capabilities.passkey_configured) {
        // Passkey available
        showSection('passkey-section');
    } else {
        // Funnel active but no passkey configured
        showSection('setup-section');
    }
}

// Setup event listeners
function setupEventListeners() {
    // Login form
    document.getElementById('login-form').addEventListener('submit', handleLogin);

    // Passkey button
    document.getElementById('btn-passkey').addEventListener('click', () => {
        startPasskeyLogin();
    });

    // Setup button
    document.getElementById('btn-setup-passkey').addEventListener('click', () => {
        startSetup();
    });

    // Cancel QR
    document.getElementById('btn-cancel-qr').addEventListener('click', () => {
        updateUI();
    });

    // Recovery buttons
    document.getElementById('btn-recovery').addEventListener('click', handleRecovery);
    document.getElementById('btn-back-to-login').addEventListener('click', () => {
        updateUI();
    });

    // Recovery code display
    document.getElementById('btn-saved-codes').addEventListener('click', () => {
        showSuccess('Passkey setup complete! You can now use it to login.');
        updateUI();
    });

    // Download recovery links
    document.getElementById('btn-download-recovery').addEventListener('click', (e) => {
        e.preventDefault();
        window.location.href = `${API_BASE}/auth/recovery/download`;
    });
    document.getElementById('btn-download-new-recovery').addEventListener('click', (e) => {
        e.preventDefault();
        window.location.href = `${API_BASE}/auth/recovery/download`;
    });
}

// Handle password login
async function handleLogin(e) {
    e.preventDefault();
    const username = document.getElementById('username').value;
    const password = document.getElementById('password').value;

    clearMessages();

    try {
        const response = await fetch(`${API_BASE}/login`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ username, password })
        });

        const data = await response.json();

        if (response.ok) {
            // Login successful
            if (data.requires_password_change) {
                showSuccess('Login successful. Please change your password.');
                // Redirect to password change page
                window.location.href = '/password.html';
            } else {
                showSuccess('Login successful! Redirecting...');
                setTimeout(() => {
                    window.location.href = '/';
                }, 1000);
            }
        } else {
            showError(data || 'Login failed');
        }
    } catch (error) {
        showError('Network error. Please try again.');
    }
}

// Start passkey login
async function startPasskeyLogin() {
    showSection('qr-section');
    document.getElementById('btn-cancel-qr').innerHTML = '<span class="spinner"></span> Generating challenge...';
    document.getElementById('btn-cancel-qr').disabled = true;

    try {
        const response = await fetch(`${API_BASE}/passkey/login/challenge`, {
            method: 'POST'
        });

        const data = await response.json();

        if (data.success === false) {
            showError(data.error || 'Failed to generate challenge');
            return;
        }

        // For now, show the enrollment URL as a link (actual WebAuthn requires HTTPS and proper setup)
        const qrUrl = `${capabilities.funnel_url}/passkey/enroll/${data.challenge_id}`;
        const qrContainer = document.getElementById('qr-image');
        qrContainer.src = `${API_BASE}/qr?text=${encodeURIComponent(qrUrl)}`;

        document.getElementById('btn-cancel-qr').innerHTML = 'Cancel';
        document.getElementById('btn-cancel-qr').disabled = false;

        // Poll for passkey verification (simplified)
        pollForPasskeyVerification(data.challenge_id);

    } catch (error) {
        showError('Failed to start passkey login');
        console.error(error);
    }
}

// Start setup flow
async function startSetup() {
    hideAllSections();
    showSection('qr-section');
    document.getElementById('btn-cancel-qr').innerHTML = '<span class="spinner"></span> Setting up...';
    document.getElementById('btn-cancel-qr').disabled = true;

    try {
        const response = await fetch(`${API_BASE}/passkey/setup`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ device_name: 'Phone' })
        });

        const data = await response.json();

        if (!data.success) {
            showError(data.error || 'Failed to setup passkey');
            return;
        }

        // Display QR code
        const qrContainer = document.getElementById('qr-image');
        qrContainer.src = data.qr_code;

        document.getElementById('btn-cancel-qr').innerHTML = 'Cancel';
        document.getElementById('btn-cancel-qr').disabled = false;

        // Poll for enrollment completion
        await pollForEnrollment();

    } catch (error) {
        showError('Failed to setup passkey');
        console.error(error);
    }
}

// Poll for enrollment completion
async function pollForEnrollment() {
    const maxAttempts = 60; // 5 minutes max
    let attempts = 0;

    const poll = async () => {
        attempts++;

        // For enrollment, we wait for the user to complete the flow
        // In a real implementation, this would poll for completion
        // For now, we'll check if the passkey is now configured

        await new Promise(resolve => setTimeout(resolve, 5000));

        await checkCapabilities();

        if (capabilities.passkey_configured) {
            // Passkey is now configured, show recovery codes
            showSection('recovery-display-section');
            await showRecoveryCodes();
        } else if (attempts < maxAttempts) {
            await poll();
        } else {
            showError('Enrollment timed out. Please try again.');
        }
    };

    await poll();
}

// Poll for passkey verification
async function pollForPasskeyVerification(challengeId) {
    const maxAttempts = 30; // 2.5 minutes max
    let attempts = 0;

    const poll = async () => {
        attempts++;

        await new Promise(resolve => setTimeout(resolve, 2000));

        // Check if we can verify (in a real implementation, we'd check session state)
        // For now, we'll just redirect on success

        await checkCapabilities();

        if (currentState === 'recovery_display') {
            return; // Already handled
        }

        if (attempts < maxAttempts) {
            await poll();
        } else {
            showError('Passkey verification timed out. Please try again.');
        }
    };

    await poll();
}

// Show recovery codes
async function showRecoveryCodes() {
    // Recovery codes are shown after successful enrollment
    // For now, just show a placeholder
    document.getElementById('recovery-codes').innerHTML = 'Recovery codes will appear here after enrollment.';
}

// Handle recovery code
async function handleRecovery() {
    const code = document.getElementById('recovery-code').value.trim();
    if (!code) {
        showError('Please enter a recovery code');
        return;
    }

    clearMessages();

    try {
        const response = await fetch(`${API_BASE}/auth/recover`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({ recovery_code: code })
        });

        const data = await response.json();

        if (data.success) {
            showSuccess('Recovery successful! Redirecting...');
            setTimeout(() => {
                window.location.href = '/';
            }, 1000);
        } else {
            showError(data.error || 'Invalid recovery code');
        }
    } catch (error) {
        showError('Network error. Please try again.');
    }
}

// UI Helper functions
function hideAllSections() {
    const sections = ['password-section', 'passkey-section', 'setup-section',
                      'tailscale-missing-section', 'qr-section', 'recovery-section',
                      'recovery-display-section'];
    sections.forEach(id => {
        const el = document.getElementById(id);
        if (el) el.style.display = 'none';
    });
}

function showSection(id) {
    const el = document.getElementById(id);
    if (el) {
        el.style.display = 'block';
        currentState = id.replace('-section', '');
    }
}

function showPasswordOnly() {
    hideAllSections();
    showSection('password-section');
}

function showError(message) {
    const el = document.getElementById('login-error');
    el.textContent = message;
    el.style.display = 'block';
    document.getElementById('login-success').style.display = 'none';
}

function showSuccess(message) {
    const el = document.getElementById('login-success');
    el.textContent = message;
    el.style.display = 'block';
    document.getElementById('login-error').style.display = 'none';
}

function clearMessages() {
    document.getElementById('login-error').style.display = 'none';
    document.getElementById('login-success').style.display = 'none';
}

// Check SD card health
async function checkSdHealth() {
    try {
        const response = await fetch(`${API_BASE}/storage/health/status`);
        if (response.ok) {
            const data = await response.json();
            const healthEl = document.getElementById('sd-health');
            const dotEl = document.getElementById('sd-health-dot');
            const textEl = document.getElementById('sd-health-text');

            const statusColors = {
                'GOOD': '#00d9a0',
                'FAIR': '#f39c12',
                'WARNING': '#e67e22',
                'FAIL': '#e94560',
                'UNKNOWN': '#666'
            };

            healthEl.style.display = 'inline-flex';
            dotEl.style.backgroundColor = statusColors[data.status] || statusColors['UNKNOWN'];
            textEl.textContent = `SD: ${data.status}`;
        }
    } catch (error) {
        console.log('SD health check unavailable');
    }
}

// Initialize SD health check
checkSdHealth();

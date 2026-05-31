// ================================================================
// STEALTH ADDRESS & BLE MESH INTEGRATION
// Privacy-preserving P2P payments with offline mesh networking
// ================================================================

// Stealth State Management
const stealthState = {
    metaAddress: null,
    stealthKeyPair: null,
    detectedPayments: [],
    queuedPayments: [],
    meshStatus: {
        connected: false,
        peers: 0,
        packetsRelayed: 0
    },
    scanningActive: false
};

// Initialize Stealth View
function initializeStealthView() {
    setupStealthListeners();
    loadStealthConfiguration();
    checkMeshStatus();
    
    // Auto-scan if wallet connected
    if (state.connectedWallet) {
        startAutoScanning();
    }
}

// Setup Event Listeners
function setupStealthListeners() {
    // Generate stealth address
    document.getElementById('generateStealthAddress')?.addEventListener('click', generateStealthAddress);
    
    // Prepare stealth payment
    document.getElementById('prepareStealthPayment')?.addEventListener('click', prepareStealthPayment);
    
    // Send stealth payment
    document.getElementById('sendStealthPayment')?.addEventListener('click', sendStealthPayment);
    
    // Scan for payments
    document.getElementById('scanStealthPayments')?.addEventListener('click', scanForPayments);
    
    // Toggle auto-scan
    document.getElementById('toggleAutoScan')?.addEventListener('click', toggleAutoScanning);
    
    // Shield/Unshield
    document.getElementById('shieldFunds')?.addEventListener('click', shieldFunds);
    document.getElementById('unshieldFunds')?.addEventListener('click', unshieldFunds);
    
    // QR Code
    document.getElementById('showStealthQR')?.addEventListener('click', showStealthQR);
    document.getElementById('scanStealthQR')?.addEventListener('click', scanStealthQR);
    
    // Payment Queue
    document.getElementById('refreshPaymentQueue')?.addEventListener('click', refreshPaymentQueue);
    
    // Mesh Network
    document.getElementById('toggleMeshNetwork')?.addEventListener('click', toggleMeshNetwork);
    document.getElementById('refreshMeshStatus')?.addEventListener('click', refreshMeshStatus);
}

// ================================================================
// STEALTH ADDRESS GENERATION
// ================================================================

async function generateStealthAddress() {
    const errorDiv = document.getElementById('stealthError');
    errorDiv.textContent = '';
    
    showLoading();
    
    try {
        const response = await fetch(`${API_BASE_URL}/api/stealth/generate`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
                version: 1 // Standard mode
            })
        });
        
        if (!response.ok) {
            const error = await response.json();
            throw new Error(error.error || 'Failed to generate stealth address');
        }
        
        const data = await response.json();
        stealthState.metaAddress = data.data.meta_address;
        
        // Display meta-address
        document.getElementById('stealthMetaAddress').textContent = stealthState.metaAddress;
        document.getElementById('stealthAddressSection').classList.remove('hidden');
        
        showToast('Stealth address generated!', 'success');
        
    } catch (error) {
        console.error('Error generating stealth address:', error);
        errorDiv.textContent = error.message;
        showToast('Failed to generate stealth address', 'error');
    } finally {
        hideLoading();
    }
}

// ================================================================
// STEALTH PAYMENTS
// ================================================================

async function prepareStealthPayment() {
    const receiverMeta = document.getElementById('receiverMetaAddress').value.trim();
    const amount = parseFloat(document.getElementById('stealthPaymentAmount').value);
    const errorDiv = document.getElementById('stealthPaymentError');
    
    errorDiv.textContent = '';
    
    if (!receiverMeta || !amount || amount <= 0) {
        errorDiv.textContent = 'Please enter receiver meta-address and amount';
        return;
    }
    
    showLoading();
    
    try {
        const response = await fetch(`${API_BASE_URL}/api/stealth/prepare-payment`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
                receiver_meta_address: receiverMeta,
                amount_lamports: Math.floor(amount * 1e9) // Convert SOL to lamports
            })
        });
        
        if (!response.ok) {
            const error = await response.json();
            throw new Error(error.error || 'Failed to prepare payment');
        }
        
        const data = await response.json();
        const prepared = data.data;
        
        // Display prepared payment details
        document.getElementById('preparedStealthAddress').textContent = prepared.stealth_address;
        document.getElementById('preparedEphemeralKey').textContent = prepared.ephemeral_public_key;
        document.getElementById('preparedViewingTag').textContent = 
            Array.from(prepared.viewing_tag).map(b => b.toString(16).padStart(2, '0')).join('');
        document.getElementById('preparedAmount').textContent = (prepared.amount / 1e9).toFixed(4);
        
        document.getElementById('preparedPaymentSection').classList.remove('hidden');
        document.getElementById('sendStealthPayment').disabled = false;
        
        // Store prepared payment
        stealthState.preparedPayment = prepared;
        
        showToast('Payment prepared!', 'success');
        
    } catch (error) {
        console.error('Error preparing payment:', error);
        errorDiv.textContent = error.message;
        showToast('Failed to prepare payment', 'error');
    } finally {
        hideLoading();
    }
}

async function sendStealthPayment() {
    if (!stealthState.preparedPayment) {
        showToast('No payment prepared', 'error');
        return;
    }
    
    const useMesh = document.getElementById('useMeshNetwork')?.checked || false;
    const errorDiv = document.getElementById('stealthPaymentError');
    errorDiv.textContent = '';
    
    showLoading();
    
    try {
        const response = await fetch(`${API_BASE_URL}/api/stealth/send`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
                prepared_payment: stealthState.preparedPayment,
                use_mesh: useMesh
            })
        });
        
        if (!response.ok) {
            const error = await response.json();
            throw new Error(error.error || 'Failed to send payment');
        }
        
        const data = await response.json();
        const status = data.data.status;
        
        if (status === 'settled') {
            showToast(`Payment sent! Signature: ${data.data.signature}`, 'success');
        } else if (status === 'queued') {
            showToast('Payment queued (offline) - will settle automatically', 'warning');
            refreshPaymentQueue();
        }
        
        // Clear prepared payment
        stealthState.preparedPayment = null;
        document.getElementById('preparedPaymentSection').classList.add('hidden');
        document.getElementById('receiverMetaAddress').value = '';
        document.getElementById('stealthPaymentAmount').value = '';
        
    } catch (error) {
        console.error('Error sending payment:', error);
        errorDiv.textContent = error.message;
        showToast('Failed to send payment', 'error');
    } finally {
        hideLoading();
    }
}

// ================================================================
// SCANNING FOR PAYMENTS
// ================================================================

async function scanForPayments() {
    if (!stealthState.metaAddress) {
        showToast('Generate a stealth address first', 'warning');
        return;
    }
    
    showLoading();
    
    try {
        // Route through apiFetch for the uniform 10s timeout + error path (Req 2.8).
        const data = await apiFetch('/api/stealth/scan', {
            method: 'POST',
            body: JSON.stringify({
                meta_address: stealthState.metaAddress
            })
        });
        
        stealthState.detectedPayments = data.data.payments || [];
        
        displayDetectedPayments(stealthState.detectedPayments);
        showToast(`Found ${stealthState.detectedPayments.length} payments`, 'success');
        
    } catch (error) {
        console.error('Error scanning for payments:', error);
        showToast('Failed to scan for payments', 'error');
    } finally {
        hideLoading();
    }
}

function displayDetectedPayments(payments) {
    const container = document.getElementById('detectedPaymentsList');
    
    if (!payments || payments.length === 0) {
        container.innerHTML = '<p class="empty-state">No payments detected</p>';
        return;
    }
    
    container.innerHTML = '';
    
    payments.forEach(payment => {
        const paymentCard = document.createElement('div');
        paymentCard.className = 'card';
        paymentCard.innerHTML = `
            <div class="card-header">
                <h4>Payment Detected</h4>
                <span class="badge">${(payment.amount / 1e9).toFixed(4)} SOL</span>
            </div>
            <div class="payment-details">
                <div class="detail-row">
                    <span class="label">Stealth Address:</span>
                    <span class="value mono">${payment.stealth_address}</span>
                </div>
                <div class="detail-row">
                    <span class="label">Ephemeral Key:</span>
                    <span class="value mono">${payment.ephemeral_public_key}</span>
                </div>
                <div class="detail-row">
                    <span class="label">Viewing Tag:</span>
                    <span class="value mono">${Array.from(payment.viewing_tag).map(b => b.toString(16).padStart(2, '0')).join('')}</span>
                </div>
                <div class="detail-row">
                    <span class="label">Slot:</span>
                    <span class="value">${payment.slot}</span>
                </div>
            </div>
            <button class="btn btn-primary btn-sm" onclick="unshieldPayment('${payment.stealth_address}')">
                Unshield to Regular Address
            </button>
        `;
        container.appendChild(paymentCard);
    });
}

function toggleAutoScanning() {
    stealthState.scanningActive = !stealthState.scanningActive;
    
    const btn = document.getElementById('toggleAutoScan');
    if (stealthState.scanningActive) {
        btn.textContent = 'Stop Auto-Scan';
        btn.classList.add('active');
        startAutoScanning();
        showToast('Auto-scanning enabled', 'success');
    } else {
        btn.textContent = 'Start Auto-Scan';
        btn.classList.remove('active');
        stopAutoScanning();
        showToast('Auto-scanning disabled', 'warning');
    }
}

function startAutoScanning() {
    if (stealthState.scanInterval) {
        clearInterval(stealthState.scanInterval);
    }
    
    stealthState.scanningActive = true;
    stealthState.scanInterval = setInterval(() => {
        if (stealthState.metaAddress && stealthState.scanningActive) {
            scanForPayments();
        }
    }, 30000); // Scan every 30 seconds
}

function stopAutoScanning() {
    if (stealthState.scanInterval) {
        clearInterval(stealthState.scanInterval);
        stealthState.scanInterval = null;
    }
    stealthState.scanningActive = false;
}

// ================================================================
// SHIELD / UNSHIELD OPERATIONS
// ================================================================

async function shieldFunds() {
    const amount = parseFloat(document.getElementById('shieldAmount').value);
    const errorDiv = document.getElementById('shieldError');
    
    errorDiv.textContent = '';
    
    if (!amount || amount <= 0) {
        errorDiv.textContent = 'Please enter amount to shield';
        return;
    }
    
    if (!state.connectedWallet) {
        errorDiv.textContent = 'Connect wallet first';
        return;
    }
    
    showLoading();
    
    try {
        const response = await fetch(`${API_BASE_URL}/api/stealth/shield`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
                wallet_address: state.connectedWallet,
                amount_lamports: Math.floor(amount * 1e9)
            })
        });
        
        if (!response.ok) {
            const error = await response.json();
            throw new Error(error.error || 'Failed to shield funds');
        }
        
        const data = await response.json();
        showToast(`Funds shielded! Signature: ${data.data.signature}`, 'success');
        
        document.getElementById('shieldAmount').value = '';
        
        // Refresh portfolio
        if (state.connectedWallet) {
            await loadPortfolio(state.connectedWallet);
        }
        
    } catch (error) {
        console.error('Error shielding funds:', error);
        errorDiv.textContent = error.message;
        showToast('Failed to shield funds', 'error');
    } finally {
        hideLoading();
    }
}

async function unshieldFunds() {
    const stealthAddress = document.getElementById('unshieldStealthAddress').value.trim();
    const destination = document.getElementById('unshieldDestination').value.trim();
    const errorDiv = document.getElementById('unshieldError');
    
    errorDiv.textContent = '';
    
    if (!stealthAddress || !destination) {
        errorDiv.textContent = 'Please enter stealth address and destination';
        return;
    }
    
    showLoading();
    
    try {
        const response = await fetch(`${API_BASE_URL}/api/stealth/unshield`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
                stealth_address: stealthAddress,
                destination_address: destination
            })
        });
        
        if (!response.ok) {
            const error = await response.json();
            throw new Error(error.error || 'Failed to unshield funds');
        }
        
        const data = await response.json();
        showToast(`Funds unshielded! Signature: ${data.data.signature}`, 'success');
        
        document.getElementById('unshieldStealthAddress').value = '';
        document.getElementById('unshieldDestination').value = '';
        
    } catch (error) {
        console.error('Error unshielding funds:', error);
        errorDiv.textContent = error.message;
        showToast('Failed to unshield funds', 'error');
    } finally {
        hideLoading();
    }
}

async function unshieldPayment(stealthAddress) {
    const destination = prompt('Enter destination address:');
    if (!destination) return;
    
    document.getElementById('unshieldStealthAddress').value = stealthAddress;
    document.getElementById('unshieldDestination').value = destination;
    await unshieldFunds();
}

// ================================================================
// QR CODE SUPPORT
// ================================================================

async function showStealthQR() {
    if (!stealthState.metaAddress) {
        showToast('Generate a stealth address first', 'warning');
        return;
    }
    
    try {
        const response = await fetch(`${API_BASE_URL}/api/stealth/qr-encode`, {
            method: 'POST',
            headers: { 'Content-Type': 'application/json' },
            body: JSON.stringify({
                meta_address: stealthState.metaAddress
            })
        });
        
        if (!response.ok) {
            throw new Error('Failed to generate QR code');
        }
        
        const blob = await response.blob();
        const url = URL.createObjectURL(blob);
        
        // Display QR code in modal
        const modal = document.createElement('div');
        modal.className = 'modal';
        modal.innerHTML = `
            <div class="modal-content">
                <h3>Stealth Address QR Code</h3>
                <img src="${url}" alt="Stealth Address QR Code" style="max-width: 400px; margin: 2rem auto; display: block;">
                <p class="text-muted">Share this QR code to receive stealth payments</p>
                <button class="btn btn-primary" onclick="this.closest('.modal').remove()">Close</button>
            </div>
        `;
        document.body.appendChild(modal);
        
    } catch (error) {
        console.error('Error generating QR code:', error);
        showToast('Failed to generate QR code', 'error');
    }
}

async function scanStealthQR() {
    // In a real implementation, this would use device camera
    // For now, allow file upload
    const input = document.createElement('input');
    input.type = 'file';
    input.accept = 'image/*';
    
    input.onchange = async (e) => {
        const file = e.target.files[0];
        if (!file) return;
        
        const formData = new FormData();
        formData.append('qr_image', file);
        
        try {
            const response = await fetch(`${API_BASE_URL}/api/stealth/qr-decode`, {
                method: 'POST',
                body: formData
            });
            
            if (!response.ok) {
                throw new Error('Failed to decode QR code');
            }
            
            const data = await response.json();
            document.getElementById('receiverMetaAddress').value = data.data.meta_address;
            showToast('QR code scanned successfully!', 'success');
            
        } catch (error) {
            console.error('Error decoding QR code:', error);
            showToast('Failed to decode QR code', 'error');
        }
    };
    
    input.click();
}

// ================================================================
// PAYMENT QUEUE
// ================================================================

async function refreshPaymentQueue() {
    showLoading();
    
    try {
        // Route through apiFetch for the uniform 10s timeout + error path (Req 2.8).
        const data = await apiFetch('/api/stealth/queue');
        stealthState.queuedPayments = data.data.payments || [];
        
        displayPaymentQueue(stealthState.queuedPayments);
        
    } catch (error) {
        console.error('Error loading payment queue:', error);
        showToast('Failed to load payment queue', 'error');
    } finally {
        hideLoading();
    }
}

function displayPaymentQueue(payments) {
    const container = document.getElementById('paymentQueueList');
    
    if (!payments || payments.length === 0) {
        container.innerHTML = '<p class="empty-state">No queued payments</p>';
        return;
    }
    
    container.innerHTML = '';
    
    payments.forEach(payment => {
        const statusClass = {
            'queued': 'warning',
            'settling': 'info',
            'settled': 'success',
            'failed': 'error'
        }[payment.status] || 'default';
        
        const paymentItem = document.createElement('div');
        paymentItem.className = 'queue-item';
        paymentItem.innerHTML = `
            <div class="queue-item-header">
                <span class="payment-id mono">${payment.id}</span>
                <span class="badge badge-${statusClass}">${payment.status.toUpperCase()}</span>
            </div>
            <div class="queue-item-details">
                <div class="detail-row">
                    <span class="label">Amount:</span>
                    <span class="value">${(payment.amount / 1e9).toFixed(4)} SOL</span>
                </div>
                <div class="detail-row">
                    <span class="label">Stealth Address:</span>
                    <span class="value mono">${payment.stealth_address}</span>
                </div>
                <div class="detail-row">
                    <span class="label">Created:</span>
                    <span class="value">${new Date(payment.created_at).toLocaleString()}</span>
                </div>
                ${payment.signature ? `
                <div class="detail-row">
                    <span class="label">Signature:</span>
                    <span class="value mono">${payment.signature}</span>
                </div>
                ` : ''}
            </div>
        `;
        container.appendChild(paymentItem);
    });
}

// ================================================================
// BLE MESH NETWORK
// ================================================================

async function checkMeshStatus() {
    try {
        // Route through apiFetch for the uniform 10s timeout + error path (Req 2.8).
        const data = await apiFetch('/api/mesh/status');
        stealthState.meshStatus = data.data;
        
        displayMeshStatus(stealthState.meshStatus);
        
    } catch (error) {
        console.error('Error checking mesh status:', error);
        // Mesh might not be available, that's okay
    }
}

function displayMeshStatus(status) {
    const indicator = document.getElementById('meshStatusIndicator');
    const peerCount = document.getElementById('meshPeerCount');
    const packetsRelayed = document.getElementById('meshPacketsRelayed');
    
    if (indicator) {
        indicator.className = status.connected ? 'status-indicator healthy' : 'status-indicator unhealthy';
    }
    
    if (peerCount) {
        peerCount.textContent = status.peers || 0;
    }
    
    if (packetsRelayed) {
        packetsRelayed.textContent = status.packetsRelayed || 0;
    }
}

async function toggleMeshNetwork() {
    const action = stealthState.meshStatus.connected ? 'disconnect' : 'connect';
    
    showLoading();
    
    try {
        const response = await fetch(`${API_BASE_URL}/api/mesh/${action}`, {
            method: 'POST'
        });
        
        if (!response.ok) {
            throw new Error(`Failed to ${action} mesh network`);
        }
        
        await checkMeshStatus();
        showToast(`Mesh network ${action}ed`, 'success');
        
    } catch (error) {
        console.error(`Error ${action}ing mesh:`, error);
        showToast(`Failed to ${action} mesh network`, 'error');
    } finally {
        hideLoading();
    }
}

async function refreshMeshStatus() {
    await checkMeshStatus();
    showToast('Mesh status refreshed', 'success');
}

// ================================================================
// CONFIGURATION
// ================================================================

function loadStealthConfiguration() {
    // Load saved configuration from localStorage
    const savedMeta = localStorage.getItem('stealthMetaAddress');
    if (savedMeta) {
        stealthState.metaAddress = savedMeta;
        document.getElementById('stealthMetaAddress').textContent = savedMeta;
        document.getElementById('stealthAddressSection').classList.remove('hidden');
    }
}

function saveStealthConfiguration() {
    if (stealthState.metaAddress) {
        localStorage.setItem('stealthMetaAddress', stealthState.metaAddress);
    }
}

// Auto-save configuration on changes
window.addEventListener('beforeunload', saveStealthConfiguration);

// Export functions for global access
window.stealthFunctions = {
    initializeStealthView,
    generateStealthAddress,
    prepareStealthPayment,
    sendStealthPayment,
    scanForPayments,
    toggleAutoScanning,
    shieldFunds,
    unshieldFunds,
    showStealthQR,
    scanStealthQR,
    refreshPaymentQueue,
    toggleMeshNetwork,
    refreshMeshStatus
};

use reqwest::{Client, Error, Response};
use serde::{Deserialize, Serialize};
use rand_chacha::rand_core::SeedableRng;
use std::borrow::Borrow;

use ark_ec::{*};
use ark_ff::{*};
use ark_bw6_761::{*};
use ark_r1cs_std::prelude::*;
use ark_std::{*, rand::RngCore};
use ark_relations::r1cs::*;
use ark_groth16::{Groth16, ProvingKey, VerifyingKey};
use ark_snark::SNARK;

use lib_mpc_zexe::utils;
use lib_mpc_zexe::{vector_commitment, record_commitment, prf};
use lib_mpc_zexe::vector_commitment::bytes::{*, constraints::*};
use lib_mpc_zexe::record_commitment::{*, constraints::*};
use lib_mpc_zexe::prf::{*, constraints::*};
use lib_mpc_zexe::coin::*;

pub type ConstraintF = ark_bw6_761::Fr;

use lib_mpc_zexe::coin::*;
use lib_mpc_zexe::record_commitment::*;
use lib_mpc_zexe::encoding::*;


#[derive(Debug, Serialize, Deserialize, Clone)]
struct Order {
    id: i32,
    coin: CoinBs58,
}

#[derive(Debug, Serialize, Deserialize)]
struct Orders {
    orders: Vec<Order>,
}

async fn list_orders() -> reqwest::Result<()> {
    let client = Client::new();
    let response: Response = client.get("http://127.0.0.1:8080/debug").send().await?;
    
    if response.status().is_success() {
        let response_content: String = response.text().await?;
        println!("List of orders: {}", response_content);
    } else {
        println!("Failed to retrieve orders: {:?}", response.status());
    }
    
    Ok(())
}

async fn submit_order(item: Order) -> reqwest::Result<()> {
    let client = Client::new();
    let response = client.post("http://127.0.0.1:8080/submit")
        .json(&item)
        .send()
        .await?;
    
    if response.status().is_success() {
        println!("Item created successfully");
    } else {
        println!("Failed to create item: {:?}", response.status());
    }
    
    Ok(())
}

async fn perform_lottery() -> reqwest::Result<()> {
    let client = Client::new();
    let response = client.post("http://127.0.0.1:8080/lottery")
        .send()
        .await?;
    
    if response.status().is_success() {
        println!("Lottery executed successfully");
    } else {
        println!("Failed to execute lottery: {:?}", response.status());
    }
    
    Ok(())
}

pub struct SpendCircuit {
    pub prf_instance_nullifier: JZPRFInstance,
    pub prf_instance_ownership: JZPRFInstance,
    pub record: JZRecord<8>,
    pub coins: Vec<Coin<ark_bls12_377::Fr>>,
    pub db: JZVectorDB<ark_bls12_377::G1Affine>,
    pub index: usize,
}

fn circuit_setup() -> (ProvingKey<BW6_761>, VerifyingKey<BW6_761>) {
    let seed = [0u8; 32];
    let mut rng = rand_chacha::ChaCha8Rng::from_seed(seed);

    let circuit = setup_witness();

    let (pk, vk) = Groth16::<BW6_761>::
        circuit_specific_setup(circuit, &mut rng)
        .unwrap();

    (pk, vk)
}

fn setup_witness() -> SpendCircuit {
    let seed = [0u8; 32];
    let mut rng = rand_chacha::ChaCha8Rng::from_seed(seed);

    let prf_params = JZPRFParams::trusted_setup(&mut rng);
    let crs = JZKZGCommitmentParams::<8>::trusted_setup(&mut rng);

    let mut entropy = [0u8; 24];
    rng.fill_bytes(&mut entropy);

    let mut blind = [0u8; 24];
    rng.fill_bytes(&mut blind);

    let mut coins = Vec::new();
    let mut records = Vec::new();
    for i in 0..2u8 {
        let mut entropy = [0u8; 24];
        rng.fill_bytes(&mut entropy);
    
        let mut blind = [0u8; 24];
        rng.fill_bytes(&mut blind);

        let pubk = if i == 0 { alice_key().1 } else { bob_key().1 };
        let amount = if i == 0 { 15u8 } else { 22u8 };

        let fields: [Vec<u8>; 8] = 
        [
            entropy.to_vec(),
            pubk.to_vec(), //owner
            vec![1u8], //asset id
            vec![amount], //amount
            vec![AppId::LOTTERY as u8], //app id
            vec![0u8],
            vec![0u8],
            vec![0u8; 32],
        ];

        let coin = JZRecord::<8>::new(&crs, &fields, &blind.to_vec());
        records.push(coin.commitment().into_affine());
        coins.push(coin);
    }

    let vc_params = JZVectorCommitmentParams::trusted_setup(&mut rng);
    let db = JZVectorDB::<ark_bls12_377::G1Affine>::new(&vc_params, &records);

    SpendCircuit {
        prf_instance_ownership: JZPRFInstance::new(
            &prf_params, &[0u8; 32], &alice_key().0
        ),
        prf_instance_nullifier: JZPRFInstance::new(
            &prf_params, coins[0].fields[RHO].as_slice(), &alice_key().0
        ),
        record: coins[0].clone(),
        coins: coins.iter().map(|coin| coin.fields()).collect(),
        db: db,
        index: 0,
    }
}

impl ConstraintSynthesizer<ConstraintF> for SpendCircuit {
    //#[tracing::instrument(target = "r1cs", skip(self, cs))]
    fn generate_constraints(
        self,
        cs: ConstraintSystemRef<ConstraintF>,
    ) -> Result<()> {

        //--------------- Private key ------------------

        let params_var = JZPRFParamsVar::new_constant(
            cs.clone(),
            &self.prf_instance_ownership.params
        ).unwrap();

        let prf_instance_var = JZPRFInstanceVar::new_witness(
            cs.clone(),
            || Ok(self.prf_instance_ownership)
        ).unwrap();

        prf::constraints::generate_constraints(
            cs.clone(), &params_var, &prf_instance_var
        );

        //--------------- KZG proof ------------------

        let crs_var = JZKZGCommitmentParamsVar::<8>::new_constant(
            cs.clone(),
            self.record.crs.clone()
        ).unwrap();
        
        let coin_var = JZRecordVar::<8>::new_witness(
            cs.clone(),
            || Ok(self.record.borrow())
        ).unwrap();

        let record = self.record.borrow();
        let computed_com = record.blinded_commitment().into_affine();

        let input_com_x = ark_bls12_377::constraints::FqVar::new_input(
            ark_relations::ns!(cs, "input_com_x"), 
            || { Ok(computed_com.x) },
        ).unwrap();

        let input_com_y = ark_bls12_377::constraints::FqVar::new_input(
            ark_relations::ns!(cs, "input_com_y"), 
            || { Ok(computed_com.y) },
        ).unwrap();

        record_commitment::constraints::generate_constraints(
            cs.clone(),
            &crs_var,
            &coin_var
        ).unwrap();

        // compute the affine var from the projective var
        let coin_com_affine = coin_var.blinded_commitment.to_affine().unwrap();

        // does the computed com match the input com?
        coin_com_affine.x.enforce_equal(&input_com_x)?;
        coin_com_affine.y.enforce_equal(&input_com_y)?;

        //--------------- Merkle tree proof ------------------

        let proof = JZVectorCommitmentOpeningProof {
            root: self.db.commitment(),
            record: self.db.get_record(self.index).clone(),
            path: self.db.proof(self.index),
        };
        
        let params_var = JZVectorCommitmentParamsVar::new_constant(
            cs.clone(),
            &self.db.vc_params
        ).unwrap();

        let proof_var = JZVectorCommitmentOpeningProofVar::new_witness(
            cs.clone(),
            || Ok(&proof)
        ).unwrap();

        let root_com_x = ark_bls12_377::constraints::FqVar::new_input(
            ark_relations::ns!(cs, "input_root_x"), 
            || { Ok(proof.root.x) },
        ).unwrap();

        let root_com_y = ark_bls12_377::constraints::FqVar::new_input(
            ark_relations::ns!(cs, "input_root_y"), 
            || { Ok(proof.root.y) },
        ).unwrap();

        proof_var.root_var.x.enforce_equal(&root_com_x)?;
        proof_var.root_var.y.enforce_equal(&root_com_y)?;

        vector_commitment::bytes::constraints::generate_constraints(
            cs.clone(), &params_var, &proof_var
        );

        // --------------- Nullifier ------------------

        let nullifier_prf_f = BigInt::<6>::from_bits_le(
            &utils::bytes_to_bits(
                &self.prf_instance_nullifier.evaluate()
            )
        );

        let nullifier_x_var = ark_bls12_377::constraints::FqVar::new_input(
            ark_relations::ns!(cs, "nullifier_prf"), 
            || { Ok(ark_bls12_377::Fq::from(nullifier_prf_f)) },
        ).unwrap();

        let params_var = JZPRFParamsVar::new_constant(
            cs.clone(),
            &self.prf_instance_nullifier.params
        ).unwrap();

        let nullifier_prf_instance_var = JZPRFInstanceVar::new_witness(
            cs.clone(),
            || Ok(self.prf_instance_nullifier)
        ).unwrap();

        prf::constraints::generate_constraints(
            cs.clone(), &params_var, &nullifier_prf_instance_var
        );

        //--------------- Binding the four ------------------

        let coin_com_affine = coin_var.commitment.to_affine().unwrap();
        // just compare the x-coordinate...that's what compressed mode stores anyways
        // see ark_ec::models::short_weierstrass::GroupAffine::to_bytes
        let mut com_byte_vars: Vec::<UInt8<ConstraintF>> = Vec::new();
        com_byte_vars.extend_from_slice(&coin_com_affine.x.to_bytes()?);

        for (i, byte_var) in com_byte_vars.iter().enumerate() {
            // the serialization impl for CanonicalSerialize does x first
            byte_var.enforce_equal(&proof_var.leaf_var[i])?;
        }

        // prove ownership of the coin. Does sk correspond to coin's pk?
        for (i, byte_var) in coin_var.fields[OWNER].iter().enumerate() {
            byte_var.enforce_equal(&prf_instance_var.output_var[i])?;
        }

        // prove PRF output of nullifier
        let mut prf_byte_vars: Vec::<UInt8<ConstraintF>> = Vec::new();
        prf_byte_vars.extend_from_slice(&nullifier_x_var.to_bytes()?);
        for (i, byte_var) in nullifier_prf_instance_var.output_var.iter().enumerate() {
            byte_var.enforce_equal(&prf_byte_vars[i])?;
        }

        Ok(())
    }
}

#[tokio::main]
async fn main() -> reqwest::Result<()> {
    let seed = [0u8; 32];
    let mut rng = rand_chacha::ChaCha8Rng::from_seed(seed);
    println!("main...");

    let _crs = JZKZGCommitmentParams::<8>::trusted_setup(&mut rng);
    println!("trusted_setup complete");

    let (pk, vk) = circuit_setup();
    println!("circuit_setup complete");
    let circuit = setup_witness();
    println!("setup_witness complete");

    let blinded_com = circuit.record.blinded_commitment().into_affine();
    let input_root = circuit.db.commitment();
    let coins = circuit.coins.clone();
    let nullifier = ConstraintF::from(
            BigInt::<6>::from_bits_le(
            &utils::bytes_to_bits(
                &circuit.prf_instance_nullifier.evaluate()
            )
        )
    );

    let public_input = vec![ 
        blinded_com.x,
        blinded_com.y,
        input_root.x,
        input_root.y,
        nullifier
    ];

    let now = std::time::Instant::now();
    let proof = Groth16::<BW6_761>::prove(&pk, circuit, &mut rng).unwrap();
    let elapsed = now.elapsed();
    println!("Prover time: {:.2?}", elapsed);

    let valid_proof = Groth16::<BW6_761>::verify(&vk, &public_input, &proof).unwrap();
    assert!(valid_proof);

    let bs58_coins = coins
        .iter()
        .map(|coin| coin_to_bs58(coin))
        .collect::<Vec<_>>();
    
    list_orders().await?;
    submit_order(Order { id: 0, coin: bs58_coins[0].clone() }).await?;
    list_orders().await?;
    submit_order(Order { id: 1, coin: bs58_coins[1].clone() }).await?;
    list_orders().await?;
    perform_lottery().await?;

    Ok(())
}

fn alice_key() -> ([u8; 32], [u8; 31]) {
    let privkey = [20u8; 32];
    let pubkey =
    [
        218, 61, 173, 102, 17, 186, 176, 174, 
        54, 64, 4, 87, 114, 16, 209, 133, 
        153, 47, 114, 88, 54, 48, 138, 7,
        136, 114, 216, 152, 205, 164, 171
    ];

    (privkey, pubkey)
}

fn bob_key() -> ([u8; 32], [u8; 31]) {
    let privkey = [25u8; 32];
    let pubkey =
    [
        217, 214, 252, 243, 200, 147, 117, 28, 
        142, 219, 58, 120, 65, 180, 251, 74, 
        234, 28, 72, 194, 161, 148, 52, 219, 
        10, 34, 21, 17, 33, 38, 77,
    ];

    (privkey, pubkey)
}